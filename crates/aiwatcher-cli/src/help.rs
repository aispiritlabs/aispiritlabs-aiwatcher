//! What `aiwatcher help` prints.
//!
//! Written out rather than derived, and the reads and writes are listed from
//! their own tables so a verb cannot exist without appearing here. That split
//! is the whole trick: the part that would drift is generated from the same
//! data the dispatcher uses, and the part that needs prose — what this program
//! *is*, and the three lines somebody needs on their first day — is prose.

use crate::commands::{read, write};

/// Print the help, for everything or for one topic.
pub fn print(topic: Option<&str>) {
    match topic {
        Some("up" | "down" | "status" | "stack") => stack(),
        Some("token") => token(),
        Some("profile" | "context") => profile(),
        Some("api") => api(),
        Some("commands") => commands(),
        _ => overview(),
    }
}

fn overview() {
    println!(
        "\
aiwatcher — observability for AI agent runs

  aiwatcher <command> key=value…        values may sit anywhere on the line
  aiwatcher help <command>              more about one of them

Getting started

  aiwatcher up                          the whole local stack, on http://127.0.0.1:8080
  aiwatcher runs list window=1h         what has been running
  aiwatcher token show                  the token an agent should present

Running an instance

  up                                    the local stack: server, log, query service
  down                                  stop what `up` left behind
  status                                what is running, and what commands talk to
  serve                                 the API and the read model (a deployment's half)
  work                                  the outbox and the reactors (the other half)

Where commands go

  profile                               instances this CLI knows about
  token                                 the credential a local instance accepts

Reading and writing

  aiwatcher help commands               every verb, with the route behind it
  api                                   any route at all, for the ones with no verb
  sql                                   the local database, when this build has one

Values every read understands

  window=15m | 6h | 7d                  how far back to look (a bare number is seconds)
  format=json                           the API's own JSON, for a script
  limit=50, after=…                     one page, and the next
  profile=prod | url=https://…          somewhere other than this machine"
    );
}

fn commands() {
    println!("Reading\n");
    for spec in read::READS {
        println!("  {:<24}  {}", spec.words.join(" "), spec.summary);
        println!("  {:<24}  {}", "", spec.path);
    }
    println!("\nWriting\n");
    for spec in write::COMMANDS {
        println!("  {:<24}  {}", spec.words.join(" "), spec.summary);
        println!("  {:<24}  {}", "", spec.path);
    }
    println!(
        "\
  prompts publish           publish a prompt version (name=… text=@file)
  prompts label             point a label at a version (name=… label=… version=…)
  events publish            publish events to the ingest route (body=@file)
  executions start          start a managed run (kind=pipeline name=…)

Anything else is `aiwatcher api`."
    );
}

fn stack() {
    println!(
        "\
aiwatcher up — the local stack

  aiwatcher up                          server + write-ahead log + query service
  aiwatcher up log=iggy                 the same, on Apache Iggy in a container
  aiwatcher up log=memory               nothing survives a restart; for a demo
  aiwatcher up port=9000                somewhere other than 8080
  aiwatcher up flow=off                 without the query service

The server runs in this process; the broker and the query service are children,
started only when asked for and stopped when this exits. Ctrl-C stops all three.

What `up` does for you on the first run: generates the local token, if there is
none, and starts the server with AIWATCHER_AUTH_MODE=local so that it is the
credential. `aiwatcher token show` prints it.

log=iggy needs Docker. Apache publishes the broker as a container image and, for
Linux only, as a release binary — there is no macOS build, and this command will
not compile one for you. Without Docker, the built-in write-ahead log is
durable, single-node and needs nothing running.

  aiwatcher down                        stop a broker container left behind
  aiwatcher status                      what is up, and where commands go"
    );
}

fn token() {
    println!(
        "\
aiwatcher token — the credential a local instance accepts

  aiwatcher token create                write a new one (refuses to overwrite)
  aiwatcher token create force          replace the one that is there
  aiwatcher token show                  print it
  aiwatcher token path                  the file it lives in

One secret in one file, `0600`, read by both halves: this CLI presents it, and a
server running AIWATCHER_AUTH_MODE=local accepts it. It authenticates as an
admin — a single-user install whose owner could not rerun their own workflow
would be a strange thing — and that rests on the server being bound to the
loopback interface, which it says out loud if it is not.

It is not an ingest token. Those stay at most an editor, because they sit in an
agent's environment; this one is generated by the person at the keyboard and
never leaves the machine."
    );
}

fn profile() {
    println!(
        "\
aiwatcher profile — which instance a command talks to

  aiwatcher profile list                what there is, and which is current
  aiwatcher profile show                where commands go right now
  aiwatcher profile set name=prod url=https://aiwatcher.example token=…
  aiwatcher profile use name=prod       switch
  aiwatcher profile remove name=prod

A machine with no profiles needs none: commands go to the instance on this
machine and present the local token. A profile is for somewhere else.

One command may go somewhere else without storing anything:

  aiwatcher runs list profile=prod
  aiwatcher runs list url=https://aiwatcher.example"
    );
}

fn api() {
    println!(
        "\
aiwatcher api — any route, including the ones with no verb

  aiwatcher api get path=/api/v1/runs status=running
  aiwatcher api post path=/api/v1/events body=@run.json
  aiwatcher api post path=/api/v1/prompts body=@-      (from a pipe)
  aiwatcher api delete path=/api/v1/curation-pipelines/nightly/schedule

The method is the word after `api`, or method=…. Everything the command does not
name for itself — path, method, body, format — is sent as a query parameter, so
an unfamiliar route is usable without this CLI knowing about it.

A body may be inline JSON, @file, or @- for standard input."
    );
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_verb_appears_in_the_help() {
        // The failure this prevents is a verb that works and is undiscoverable,
        // which is the same as one that does not exist.
        for spec in super::read::READS {
            assert!(!spec.summary.is_empty(), "{:?} has no summary", spec.words);
        }
        for spec in super::write::COMMANDS {
            assert!(!spec.summary.is_empty(), "{:?} has no summary", spec.words);
        }
    }
}
