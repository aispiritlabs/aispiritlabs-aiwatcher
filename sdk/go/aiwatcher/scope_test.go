package aiwatcher_test

import (
	"context"
	"errors"
	"fmt"
	"reflect"
	"slices"
	"testing"

	"github.com/aispiritlabs/aispiritlabs-aiwatcher/sdk/go/aiwatcher"
)

func TestClient_StartRun(t *testing.T) {
	t.Parallel()

	client, sent := newRecordingClient(t)
	run := client.StartRun("run-1", aiwatcher.RunOptions{
		ConversationID: "conversation-1",
		WorkflowRunID:  "ignored without a workflow",
		Data:           map[string]any{"caller_run_id": "run-0"},
	})
	run.End(nil)

	events := sent.sent()
	if got := sent.types(); !slices.Equal(got, []string{"run.started", "run.completed"}) {
		t.Fatalf("emitted %v", got)
	}
	started := events[0]
	if started.RunID != "run-1" || run.ID() != "run-1" || started.ConversationID != "conversation-1" {
		t.Errorf("run.started names run %q in conversation %q", started.RunID, started.ConversationID)
	}
	if started.WorkflowRunID != "" {
		t.Errorf("workflow run id %q sent without a workflow id", started.WorkflowRunID)
	}
	if !uuidV7.MatchString(started.CorrelationID) || events[1].CorrelationID != started.CorrelationID {
		t.Errorf("correlation %q then %q; want one generated id for the whole run",
			started.CorrelationID, events[1].CorrelationID)
	}
	if started.Data["caller_run_id"] != "run-0" {
		t.Errorf("run.started data %v lost the caller's fields", started.Data)
	}
	if events[1].Data["status"] != "succeeded" {
		t.Errorf("run.completed data %v", events[1].Data)
	}
}

func TestRun_End(t *testing.T) {
	t.Parallel()

	tests := []struct {
		name         string
		err          error
		expectedType string
		expectedData map[string]any
	}{
		{
			name:         "success completes the run",
			expectedType: "run.completed",
			expectedData: map[string]any{"status": "succeeded"},
		},
		{
			name:         "an error fails the run with its text",
			err:          errors.New("upstream refused"),
			expectedType: "run.failed",
			expectedData: map[string]any{"status": "failed", "error": "upstream refused"},
		},
		{
			name:         "a cancellation fails the run too",
			err:          fmt.Errorf("asking the model: %w", context.Canceled),
			expectedType: "run.failed",
			expectedData: map[string]any{"status": "failed", "error": "asking the model: context canceled"},
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			t.Parallel()

			client, sent := newRecordingClient(t)
			run := client.StartRun("run-1", aiwatcher.RunOptions{})
			run.End(tt.err)
			run.End(nil)

			events := sent.sent()
			if len(events) != 2 {
				t.Fatalf("emitted %v; a second End must be ignored", sent.types())
			}
			if events[1].EventType != tt.expectedType || !sameData(events[1].Data, tt.expectedData) {
				t.Errorf("ended with %s %v, want %s %v",
					events[1].EventType, events[1].Data, tt.expectedType, tt.expectedData)
			}
		})
	}
}

func TestRun_StartAgent(t *testing.T) {
	t.Parallel()

	client, sent := newRecordingClient(t)
	run := client.StartRun("run-1", aiwatcher.RunOptions{CorrelationID: "flow-1", WorkflowID: "assistant"})
	agent := run.StartAgent("assistant")
	agent.End(errors.New("gave up"))
	agent.End(nil)
	run.End(nil)

	if got := sent.types(); !slices.Equal(got, []string{
		"run.started", "agent.started", "agent.failed", "run.completed",
	}) {
		t.Fatalf("emitted %v", got)
	}
	for _, event := range sent.sent()[1:3] {
		if event.AgentID != "assistant" || event.WorkflowID != "assistant" || event.CorrelationID != "flow-1" {
			t.Errorf("%s lost the run's scope: %+v", event.EventType, event)
		}
		if event.CausationID != "flow-1" {
			t.Errorf("%s causation %q, want it rooted on the correlation", event.EventType, event.CausationID)
		}
	}
	if failed := sent.sent()[2]; failed.Data["error"] != "gave up" {
		t.Errorf("agent.failed data %v", failed.Data)
	}
	if sent.sent()[3].AgentID != "" {
		t.Error("the run's own end carries the agent's id")
	}
}

func TestAgent_StartLLM(t *testing.T) {
	t.Parallel()

	client, sent := newRecordingClient(t)
	agent := client.StartRun("run-1", aiwatcher.RunOptions{}).StartAgent("assistant")
	temperature, maxTokens := 0.2, 512
	attributes := map[string]any{"prompt_version": "v3"}

	call := agent.StartLLM(aiwatcher.LLMRequest{
		Model: "gemini-flash", Provider: "openrouter", CallID: "gen-1",
		Temperature: &temperature, MaxTokens: &maxTokens, Attributes: attributes,
	})
	call.FirstToken()
	call.FirstToken()
	call.End(aiwatcher.Usage{InputTokens: 812, OutputTokens: 193, FinishReason: "stop", ResponseID: "gen-1"}, nil)
	call.End(aiwatcher.Usage{}, errors.New("too late"))
	attributes["prompt_version"] = "changed after the call"

	events := sent.sent()[2:]
	if got := sent.types()[2:]; !slices.Equal(got, []string{"llm.started", "llm.first_token", "llm.completed"}) {
		t.Fatalf("emitted %v", got)
	}
	request := map[string]any{
		"call_id": "gen-1", "model": "gemini-flash", "provider": "openrouter",
		"temperature": 0.2, "max_tokens": 512, "prompt_version": "v3",
	}
	if !sameData(events[0].Data, request) || !sameData(events[1].Data, request) {
		t.Errorf("start and first token carry %v and %v, want %v", events[0].Data, events[1].Data, request)
	}
	completed := events[2].Data
	duration, ok := completed["duration_ms"].(float64)
	if !ok || duration < 0 {
		t.Errorf("duration_ms %v, want a non-negative float", completed["duration_ms"])
	}
	delete(completed, "duration_ms")
	expected := map[string]any{
		"call_id": "gen-1", "model": "gemini-flash", "provider": "openrouter",
		"temperature": 0.2, "max_tokens": 512, "prompt_version": "v3",
		"input_tokens": 812, "output_tokens": 193, "finish_reason": "stop", "response_id": "gen-1",
	}
	if !sameData(completed, expected) {
		t.Errorf("llm.completed data %v, want %v — a usage field the provider did not report is not sent",
			completed, expected)
	}
}

func TestLLMCall_End_cost(t *testing.T) {
	t.Parallel()

	client, sent := newRecordingClient(t)
	agent := client.StartRun("run-1", aiwatcher.RunOptions{}).StartAgent("assistant")

	billed := agent.StartLLM(aiwatcher.LLMRequest{Model: "gemini-flash", CallID: "gen-1"})
	billed.End(aiwatcher.Usage{InputTokens: 1_240, OutputTokens: 164, CostUSD: 0.000213}, nil)
	quiet := agent.StartLLM(aiwatcher.LLMRequest{Model: "gemini-flash", CallID: "gen-2"})
	quiet.End(aiwatcher.Usage{InputTokens: 10}, nil)

	events := sent.sent()
	completed := events[3].Data
	if completed["cost_usd"] != 0.000213 {
		t.Errorf("llm.completed cost_usd %v, want the charge the provider reported", completed["cost_usd"])
	}
	if _, reported := events[5].Data["cost_usd"]; reported {
		t.Errorf("llm.completed data %v states a cost the provider never gave", events[5].Data)
	}
}

func TestLLMCall_End_failure(t *testing.T) {
	t.Parallel()

	client, sent := newRecordingClient(t)
	agent := client.StartRun("run-1", aiwatcher.RunOptions{}).StartAgent("assistant")
	first := agent.StartLLM(aiwatcher.LLMRequest{Model: "gemini-flash"})
	second := agent.StartLLM(aiwatcher.LLMRequest{Model: "gemini-flash"})
	first.End(aiwatcher.Usage{InputTokens: 10}, errors.New("status 429"))
	second.End(aiwatcher.Usage{}, nil)

	events := sent.sent()
	failed := events[4]
	if failed.EventType != "llm.failed" || failed.Data["error"] != "status 429" {
		t.Errorf("ended with %s %v", failed.EventType, failed.Data)
	}
	if _, has := failed.Data["input_tokens"]; has {
		t.Error("a failed call reports usage")
	}
	firstID, secondID := events[2].Data["call_id"], events[3].Data["call_id"]
	if firstID == secondID || failed.Data["call_id"] != firstID {
		t.Errorf("call ids %v and %v, failure names %v; want two generated ids that pair start with end",
			firstID, secondID, failed.Data["call_id"])
	}
}

func TestAgent_StartTool(t *testing.T) {
	t.Parallel()

	client, sent := newRecordingClient(t)
	agent := client.StartRun("run-1", aiwatcher.RunOptions{}).StartAgent("assistant")
	tool := agent.StartTool("web_search", "search-1", map[string]any{"query": "cegła"})
	tool.End(errors.New("no results"))
	tool.End(nil)

	events := sent.sent()[2:]
	if got := sent.types()[2:]; !slices.Equal(got, []string{"tool.started", "tool.failed"}) {
		t.Fatalf("emitted %v", got)
	}
	started := map[string]any{"tool_name": "web_search", "call_id": "search-1", "query": "cegła"}
	if !sameData(events[0].Data, started) {
		t.Errorf("tool.started data %v, want %v", events[0].Data, started)
	}
	if events[1].Data["error"] != "no results" || events[1].Data["duration_ms"] == nil {
		t.Errorf("tool.failed data %v", events[1].Data)
	}
}

func sameData(got, want map[string]any) bool {
	return reflect.DeepEqual(got, want)
}
