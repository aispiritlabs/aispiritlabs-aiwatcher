package aiwatcher_test

import (
	"bytes"
	"encoding/json"
	"errors"
	"regexp"
	"testing"
	"time"

	"github.com/santhosh-tekuri/jsonschema/v6"

	"github.com/aispiritlabs/aispiritlabs-aiwatcher/sdk/go/aiwatcher"
)

// The contract every producer answers to, whatever its language.
const envelopeSchema = "../../../contracts/envelope.schema.json"

var uuidV7 = regexp.MustCompile(`^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$`)

func TestNewID(t *testing.T) {
	t.Parallel()

	seen := make(map[string]bool)
	previous := ""
	for range 1_000 {
		id := aiwatcher.NewID()
		if !uuidV7.MatchString(id) {
			t.Fatalf("NewID() = %q, not a version 7 uuid", id)
		}
		if seen[id] {
			t.Fatalf("NewID() repeated %q", id)
		}
		seen[id] = true
		previous = id
	}

	time.Sleep(2 * time.Millisecond)
	if later := aiwatcher.NewID(); later <= previous {
		t.Errorf("an id minted later sorts first: %q <= %q", later, previous)
	}
}

func TestEnvelope_matchesContract(t *testing.T) {
	t.Parallel()

	compiler := jsonschema.NewCompiler()
	compiler.AssertFormat()
	schema, err := compiler.Compile(envelopeSchema)
	if err != nil {
		t.Fatalf("compiling %s: %v", envelopeSchema, err)
	}

	client, sent := newRecordingClient(t)
	temperature, maxTokens := 0.2, 512
	run := client.StartRun("run-1", aiwatcher.RunOptions{
		ConversationID: "conversation-1",
		WorkflowID:     "assistant",
		WorkflowRunID:  "assistant-1",
		Data:           map[string]any{"caller_run_id": "run-0"},
	})
	agent := run.StartAgent("assistant")
	call := agent.StartLLM(aiwatcher.LLMRequest{
		Model: "gemini-flash", Provider: "openrouter", Temperature: &temperature, MaxTokens: &maxTokens,
	})
	call.FirstToken()
	call.End(aiwatcher.Usage{
		InputTokens: 812, OutputTokens: 193, CachedTokens: 400,
		FinishReason: "stop", ResponseModel: "gemini-flash-001", ResponseID: "gen-1",
	}, nil)
	failed := agent.StartLLM(aiwatcher.LLMRequest{Model: "gemini-flash"})
	failed.End(aiwatcher.Usage{}, errors.New("upstream refused"))
	tool := agent.StartTool("web_search", "", map[string]any{"query": "cegła"})
	tool.End(nil)
	agent.End(errors.New("gave up"))
	run.End(errors.New("gave up"))
	client.Emit(aiwatcher.Envelope{EventType: "custom.noted", RunID: "run-1"})

	events := sent.sent()
	if len(events) != 12 {
		t.Fatalf("sent %d events, want 12: %v", len(events), sent.types())
	}
	for _, event := range events {
		body, err := json.Marshal(event)
		if err != nil {
			t.Fatalf("marshalling %s: %v", event.EventType, err)
		}
		instance, err := jsonschema.UnmarshalJSON(bytes.NewReader(body))
		if err != nil {
			t.Fatalf("reading back %s: %v", event.EventType, err)
		}
		if err := schema.Validate(instance); err != nil {
			t.Errorf("%s breaks the contract: %v\n%s", event.EventType, err, body)
		}
	}
}
