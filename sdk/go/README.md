# aiwatcher Go SDK

Publish agent-run telemetry to aiwatcher from Go: runs, the agents inside them,
and their model and tool calls, as the envelopes in
[`contracts/envelope.schema.json`](../../contracts/envelope.schema.json).

It follows the Python and TypeScript SDKs on field names, id rules and event
types. What it does not publish yet: declared workflows, evaluations, agent
messages, and a client's count of the runs it opened per variant.

## Install

```bash
go get github.com/aispiritlabs/aispiritlabs-aiwatcher/sdk/go/aiwatcher
```

Go 1.25 or newer. The package has no dependencies outside the standard library;
the two in `go.mod` are for its tests.

## Use

```go
transport, err := aiwatcher.NewHTTPTransport("http://aiwatcher-server:8080",
	aiwatcher.WithToken(os.Getenv("AIWATCHER_TOKEN"))) // only with single sign-on on
if err != nil {
	return err
}
client, err := aiwatcher.NewClient("planner-api", aiwatcher.WithTransport(transport))
if err != nil {
	return err
}
defer client.Close(shutdownCtx) // delivers what is still queued

run := client.StartRun(aiwatcher.NewID(), aiwatcher.RunOptions{ConversationID: conversationID})
agent := run.StartAgent("assistant")
call := agent.StartLLM(aiwatcher.LLMRequest{Model: "gemini-flash", Provider: "openrouter"})

answer, err := askTheModel(ctx)

call.End(aiwatcher.Usage{InputTokens: answer.PromptTokens, OutputTokens: answer.CompletionTokens}, err)
agent.End(err)
run.End(err)
```

Every `Start` returns a handle that must be ended once; a second `End` is
ignored. A client created without a transport discards everything, so wiring
one into code under test changes nothing.

## What it guarantees, and what it does not

- **Publishing never blocks.** `Send` queues; one goroutine posts batches of
  `WithBatchSize` (64) or after `WithFlushInterval` (1 s), whichever is first.
- **Memory is bounded.** A full queue (`WithQueueSize`, 10 000) drops events and
  counts them in `Dropped()`.
- **Telemetry is not retried.** A batch the server refuses, or cannot be
  reached for, is dropped, counted and logged once through `slog`.
- **Content is the caller's decision.** The SDK sends no prompt or answer text
  unless you put it in `Attributes` yourself.
- **The numbering matches the other SDKs.** Each client counts the events it
  sends into a run from 0 under an id of its own, and forgets a run's count
  when the run ends.

## Develop

```bash
cd sdk/go
go vet ./...
go test -race ./...
```

The contract test validates every envelope the SDK produces against the JSON
Schema in `contracts/`, so a field that drifts from the wire format fails
here rather than on the server.
