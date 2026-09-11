# The decisions, in four readings

Twenty-eight ADRs is the right number of *records* and the wrong number of
*things to read*. These four documents group them by the question they answer,
say what each decided in one line, and — the part a flat index cannot give —
say which ones later ones amended or partly took back.

**The ADR is the record; this is a reading guide.** Nothing here supersedes
anything, nothing here is cited from code, and when the two disagree the ADR is
right. Every line links to the file it summarises.

| Reading | The question it answers | ADRs |
|---|---|---|
| [The log](OBSERVABILITY.md) | What is observed, how it folds, and what retention therefore bounds | 0001–0005, 0007, 0010, 0012 |
| [The registries](REGISTRIES.md) | What is authored rather than observed, and so lives outside retention | 0011, 0015, 0017–0023 |
| [Execution](EXECUTION.md) | Who runs work, who decides, and where the facts about it go | 0008, 0014, 0016, 0024–0026, 0028 |
| [Deployment](DEPLOYMENT.md) | How it is installed, and who may call it | 0006, 0009, 0013, 0027 |

The split is the one the codebase already makes. Everything in *The log* is a
fold over the event log and is bounded by its retention; everything in *The
registries* exists precisely because that bound would lose it. *Execution* is
the arc from "the browser drives a query" to "the server owns a run and
publishes facts about it". *Deployment* is the only group whose decisions a
reader of the crates never meets.

For a new decision, write an ADR from [the template](../ADR/template.md) and add
a line here. A reading that grows past a screen of table has probably absorbed a
group that wants its own file.
