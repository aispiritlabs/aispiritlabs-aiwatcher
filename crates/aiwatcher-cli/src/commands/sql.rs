//! `aiwatcher sql …` — the local database, without a server running.
//!
//! `AIWATCHER_WORKFLOW_STORE=duckdb` puts every managed run's stream, its
//! projection, its outbox and its claim table in one file, and the reason that
//! adapter exists at all is half performance and half this: a directory of JSON
//! is something nobody can ask a question of. So the answer to "what did that
//! run actually decide" is a `select`, and this is the command that runs it.
//!
//! ## Read-only, and enforced by the connection
//!
//! The database is opened with `AccessMode::ReadOnly`. That is the whole of the
//! safety story and it is deliberately not a list of refused keywords: DuckDB
//! refuses the write itself, so an `update` nobody thought of is refused for the
//! same reason `delete` is, and this file has no opinion about SQL it has never
//! heard of. The store's own rules stay in Rust either way — nothing here is a
//! second answer to "is this attempt claimable"; it is a window onto the rows
//! those answers were written into.
//!
//! DuckDB gives a database file to **one writer or to any number of readers**,
//! so this cannot look inside a store that a running instance is holding. That
//! is reported as what it is, with the way out, rather than as a file error.

use std::path::PathBuf;

use duckdb::types::ValueRef;
use duckdb::{AccessMode, Config, Connection};
use serde_json::{Map, Value};

use crate::client::emit;
use crate::paths::Paths;
use crate::{Args, CliError, Format};

/// How many rows a table renders before it says there are more.
///
/// A cap rather than a page: this is a terminal, and `limit=` in the query is
/// how somebody who wants the rest asks for them. Overridable with `limit=`.
const ROWS: usize = 200;

/// Dispatch `sql | sql tables | sql schema | sql path`.
///
/// # Errors
///
/// [`CliError::Usage`] for a word this command does not know or a query it was
/// not given, [`CliError::Environment`] when the database cannot be opened, and
/// [`CliError::Other`] when DuckDB refuses the statement.
pub fn run(args: &Args, paths: &Paths) -> Result<(), CliError> {
    let path = database(paths);

    // Before opening anything: `path` is the one subcommand that is worth an
    // answer when there is no database yet, because "where would it be" is
    // exactly what somebody asks when they cannot find it.
    if args.word(1) == Some("path") {
        println!("{}", path.display());
        return Ok(());
    }

    if let Some(query) = args.value("query") {
        return show(&open(&path)?, query, args);
    }

    match args.word(1) {
        None | Some("tables") => show(&open(&path)?, TABLES, args),
        Some("schema") => {
            let connection = open(&path)?;
            match args.value("table") {
                Some(table) => columns(&connection, table, args),
                None => show(&connection, COLUMNS, args),
            }
        }
        Some(other) => Err(CliError::Usage(format!(
            "sql {other:?}; expected tables, schema, path, or query=\"select …\""
        ))),
    }
}

/// Where a locally-run stack keeps its workflow store.
///
/// The same expression `aiwatcher_server::wiring` uses, because a CLI that
/// looked somewhere else would answer questions about a different database and
/// nothing would say so.
fn database(paths: &Paths) -> PathBuf {
    paths.data_dir.join("aiwatcher.duckdb")
}

/// Open it for reading, and explain a refusal in the terms that caused it.
fn open(path: &std::path::Path) -> Result<Connection, CliError> {
    if !path.exists() {
        return Err(CliError::Environment(format!(
            "no database at {}. A local stack writes one when it runs with \
             AIWATCHER_WORKFLOW_STORE=duckdb",
            path.display()
        )));
    }
    let config = Config::default()
        .access_mode(AccessMode::ReadOnly)
        .map_err(|error| CliError::Environment(format!("configuring DuckDB: {error}")))?;

    Connection::open_with_flags(path, config).map_err(|error| {
        CliError::Environment(format!(
            "opening {}: {error}. DuckDB gives a database to one writer or to any number of \
             readers, so a running instance holds this one — `aiwatcher down` first, or ask \
             the instance instead",
            path.display()
        ))
    })
}

/// Every table in the store, with how many rows it holds.
///
/// `duckdb_tables()` rather than a list written out here: the schema is
/// `aiwatcher-execution`'s and a copy of it in this crate would be a second
/// answer, wrong from the first migration that added a table.
const TABLES: &str = "select table_name as \"table\", estimated_size as rows \
                      from duckdb_tables() where schema_name = 'main' order by table_name";

/// Every column of every table, for when the question is what a table holds.
const COLUMNS: &str = "select table_name as \"table\", column_name as column, data_type as type \
                       from information_schema.columns where table_schema = 'main' \
                       order by table_name, ordinal_position";

/// One table's columns.
///
/// Bound rather than interpolated. The name arrives from a command line, and
/// the fact that this connection could not write even if it were a statement
/// is not a reason to build SQL out of somebody's argument.
fn columns(connection: &Connection, table: &str, args: &Args) -> Result<(), CliError> {
    rows(
        connection,
        "select column_name as column, data_type as type, is_nullable as nullable \
         from information_schema.columns where table_schema = 'main' and table_name = ? \
         order by ordinal_position",
        [table],
        args,
    )
    .and_then(|value| emit(&value, Format::from_args(args)?))
}

/// Run a statement and print what came back.
fn show(connection: &Connection, query: &str, args: &Args) -> Result<(), CliError> {
    let value = rows(connection, query, [], args)?;
    emit(&value, Format::from_args(args)?)
}

/// Read a result set into JSON, so that one renderer draws every table this CLI
/// prints and `format=json` means the same thing here as everywhere else.
fn rows<P: duckdb::Params>(
    connection: &Connection,
    query: &str,
    params: P,
    args: &Args,
) -> Result<Value, CliError> {
    let limit = args.number("limit")?.unwrap_or(ROWS);

    let mut statement = connection
        .prepare(query)
        .map_err(|error| CliError::Other(anyhow::anyhow!("{error}")))?;
    let mut result = statement
        .query(params)
        .map_err(|error| CliError::Other(anyhow::anyhow!("{error}")))?;

    let mut collected: Vec<Value> = Vec::new();
    let mut names: Option<Vec<String>> = None;
    let mut seen = 0_usize;

    while let Some(row) = result
        .next()
        .map_err(|error| CliError::Other(anyhow::anyhow!("{error}")))?
    {
        seen += 1;
        if collected.len() >= limit {
            continue;
        }
        // The column names come from the statement rather than from the row, so
        // an expression a query named itself — `count(*) as runs` — reads back
        // under the name it was given. Read once: they are the same for every
        // row of one result set.
        let names = names.get_or_insert_with(|| row.as_ref().column_names());
        let mut object = Map::with_capacity(names.len());
        for (index, name) in names.iter().enumerate() {
            let value = row
                .get_ref(index)
                .map_err(|error| CliError::Other(anyhow::anyhow!("{error}")))?;
            object.insert(name.clone(), json(value));
        }
        collected.push(Value::Object(object));
    }

    if seen > collected.len() {
        eprintln!(
            "(showing {} of {seen} rows — limit={} for more)",
            collected.len(),
            seen
        );
    }
    Ok(Value::Array(collected))
}

/// One cell as JSON.
///
/// Anything this does not have a JSON shape for becomes its own `Debug`
/// rendering rather than a null: a column somebody cannot read is a bug report,
/// and a column that silently reads as empty is not.
fn json(value: ValueRef<'_>) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Boolean(inner) => Value::Bool(inner),
        ValueRef::TinyInt(inner) => Value::from(inner),
        ValueRef::SmallInt(inner) => Value::from(inner),
        ValueRef::Int(inner) => Value::from(inner),
        ValueRef::BigInt(inner) => Value::from(inner),
        ValueRef::UTinyInt(inner) => Value::from(inner),
        ValueRef::USmallInt(inner) => Value::from(inner),
        ValueRef::UInt(inner) => Value::from(inner),
        ValueRef::UBigInt(inner) => Value::from(inner),
        ValueRef::Float(inner) => Value::from(inner),
        ValueRef::Double(inner) => Value::from(inner),
        ValueRef::Text(bytes) => match std::str::from_utf8(bytes) {
            // Every payload column in this store is JSON, so a stream's data
            // renders as the structure it is rather than as a quoted blob.
            Ok(text) => {
                serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_owned()))
            }
            Err(_) => Value::String(String::from_utf8_lossy(bytes).into_owned()),
        },
        ValueRef::Blob(bytes) => Value::String(format!("<{} bytes>", bytes.len())),
        other => Value::String(format!("{other:?}")),
    }
}
