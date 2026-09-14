// Package aiwatcher publishes agent-run telemetry to aiwatcher from Go.
//
// The contract is the envelope in contracts/envelope.schema.json, not this
// package: anything that produces that JSON and gets it to the server is a
// valid producer. This package makes the common case — a run, an agent inside
// it, a model call inside that — a handful of lines, and follows the Python and
// TypeScript SDKs on field names, id rules and event types, so a team with
// producers in several languages holds one model of them.
//
// A [Client] numbers what it sends and hands it to a [Transport]. The
// [HTTPTransport] batches envelopes to POST /api/v1/events on a goroutine of
// its own; publishing never waits on the network, and a full queue drops
// events and counts them rather than blocking the code being observed. A
// Client created without a transport discards everything, so wiring one in
// never breaks a test.
//
// Runs, agents, model calls and tool calls are handles opened with Start and
// closed with End, the way OpenTelemetry spans are:
//
//	transport, err := aiwatcher.NewHTTPTransport("http://aiwatcher-server:8080")
//	if err != nil {
//		return err
//	}
//	client, err := aiwatcher.NewClient("planner-api", aiwatcher.WithTransport(transport))
//	if err != nil {
//		return err
//	}
//	defer client.Close(ctx)
//
//	run := client.StartRun(aiwatcher.NewID(), aiwatcher.RunOptions{ConversationID: conversation})
//	agent := run.StartAgent("assistant")
//	call := agent.StartLLM(aiwatcher.LLMRequest{Model: "gemini-flash", Provider: "openrouter"})
//	answer, usage, err := ask(ctx)
//	call.End(usage, err)
//	agent.End(err)
//	run.End(err)
//
// Every handle must be ended exactly once; a second End is ignored. A start
// that never reports an end looks identical to a hang, and the server closes
// it only when its orphan sweeper gives up on it.
//
// What this package does not publish yet: declared workflows, evaluations,
// agent messages and a client's count of the runs it opened per variant. The
// Python SDK has all of them.
package aiwatcher
