package aiwatcher

import (
	"sync/atomic"
	"time"
)

// Event types this package emits. docs/event-catalog.md has the full list.
const (
	eventRunStarted     = "run.started"
	eventRunCompleted   = "run.completed"
	eventRunFailed      = "run.failed"
	eventAgentStarted   = "agent.started"
	eventAgentCompleted = "agent.completed"
	eventAgentFailed    = "agent.failed"
	eventLLMStarted     = "llm.started"
	eventLLMFirstToken  = "llm.first_token"
	eventLLMCompleted   = "llm.completed"
	eventLLMFailed      = "llm.failed"
	eventToolStarted    = "tool.started"
	eventToolCompleted  = "tool.completed"
	eventToolFailed     = "tool.failed"
)

// Run statuses the server reads from data.status.
const (
	statusSucceeded = "succeeded"
	statusFailed    = "failed"
)

// RunOptions describes a run beyond its id. Every field is optional.
type RunOptions struct {
	// ConversationID groups runs a user would think of as one conversation:
	// the explorer's session pivot.
	ConversationID string
	// WorkflowID names the orchestration this run executes.
	WorkflowID string
	// WorkflowRunID names one execution of that orchestration, when it spans
	// several runs. Ignored without WorkflowID.
	WorkflowRunID string
	// CorrelationID groups an entire flow. Generated when empty.
	CorrelationID string
	// Data is added to run.started, such as caller_run_id for a model server
	// reporting whose call it served.
	Data map[string]any
}

// Run is one execution of an agent, which the server assembles into one
// trace. Its methods are safe for concurrent use.
type Run struct {
	client *Client
	// scope holds the envelope fields every event of the scope carries.
	scope   Envelope
	isEnded atomic.Bool
}

// StartRun emits run.started and returns the run, which must be ended with
// [Run.End]. The run id is the caller's: the partition key of the log and the
// scope of the trace. [NewID] makes one when the caller has none.
func (c *Client) StartRun(runID string, opts RunOptions) *Run {
	scope := Envelope{
		RunID:          runID,
		ConversationID: opts.ConversationID,
		WorkflowID:     opts.WorkflowID,
		CorrelationID:  opts.CorrelationID,
	}
	if opts.WorkflowID != "" {
		scope.WorkflowRunID = opts.WorkflowRunID
	}
	if scope.CorrelationID == "" {
		scope.CorrelationID = NewID()
	}

	r := &Run{client: c, scope: scope}
	r.emit(eventRunStarted, withData(nil, opts.Data))
	return r
}

// ID returns the run id.
func (r *Run) ID() string {
	return r.scope.RunID
}

// StartAgent emits agent.started for an agent working inside the run and
// returns it, which must be ended with [Agent.End].
func (r *Run) StartAgent(agentID string) *Agent {
	scope := r.scope
	scope.AgentID = agentID
	// Emmett's rule: an unseeded causation roots itself on the correlation.
	if scope.CausationID == "" {
		scope.CausationID = scope.CorrelationID
	}
	a := &Agent{client: r.client, scope: scope}
	a.emit(eventAgentStarted, map[string]any{})
	return a
}

// End emits run.completed when err is nil and run.failed otherwise, with the
// error's text — a cancellation included, because a cancelled run that never
// reports an end looks identical to a hung one. Only the first End counts.
func (r *Run) End(err error) {
	if !r.isEnded.CompareAndSwap(false, true) {
		return
	}
	if err != nil {
		r.emit(eventRunFailed, map[string]any{"status": statusFailed, "error": err.Error()})
		return
	}
	r.emit(eventRunCompleted, map[string]any{"status": statusSucceeded})
}

func (r *Run) emit(eventType string, data map[string]any) {
	event := r.scope
	event.EventType = eventType
	event.Data = data
	r.client.Emit(event)
}

// Agent is an agent working inside a run: the container its model and tool
// calls nest in. Its methods are safe for concurrent use.
type Agent struct {
	client *Client
	// scope holds the envelope fields every event of the scope carries.
	scope   Envelope
	isEnded atomic.Bool
}

// End emits agent.completed when err is nil and agent.failed otherwise. Only
// the first End counts.
func (a *Agent) End(err error) {
	if !a.isEnded.CompareAndSwap(false, true) {
		return
	}
	if err != nil {
		a.emit(eventAgentFailed, map[string]any{"error": err.Error()})
		return
	}
	a.emit(eventAgentCompleted, map[string]any{})
}

func (a *Agent) emit(eventType string, data map[string]any) {
	event := a.scope
	event.EventType = eventType
	event.Data = data
	a.client.Emit(event)
}

// LLMRequest describes a model call as it is made.
type LLMRequest struct {
	// Model is the requested model: the span's name and the explorer's model
	// pivot.
	Model string
	// Provider names who serves the model, such as openai or openrouter.
	Provider string
	// CallID tells apart concurrent calls inside one agent; without one, two
	// parallel calls collapse into one span. Generated when empty — pass the
	// provider's request id where there is one, so the span joins up with the
	// provider's own logs.
	CallID string
	// Temperature and MaxTokens are the request's parameters, when it set them.
	Temperature *float64
	MaxTokens   *int
	// Attributes are added to every event of the call.
	Attributes map[string]any
}

// Usage is what a model call reported back. A zero field means the provider
// did not say, and is not sent.
type Usage struct {
	InputTokens  int
	OutputTokens int
	CachedTokens int
	// CostUSD is what the provider said the call cost, in US dollars, where it
	// says so at all — OpenRouter's `usage.cost`, for one. A charge rather than
	// an estimate from a price table, which is why it travels beside the tokens
	// instead of being derived from them. Left at zero it is not sent, and the
	// call reads as one whose cost nobody stated.
	CostUSD       float64
	FinishReason  string
	ResponseModel string
	ResponseID    string
}

// LLMCall is a model call in flight. Its methods are safe for concurrent use.
type LLMCall struct {
	agent         *Agent
	base          map[string]any
	started       time.Time
	hasFirstToken atomic.Bool
	isEnded       atomic.Bool
}

// StartLLM emits llm.started and returns the call, which must be ended with
// [LLMCall.End].
func (a *Agent) StartLLM(request LLMRequest) *LLMCall {
	callID := request.CallID
	if callID == "" {
		callID = NewID()
	}
	base := withData(request.Attributes, map[string]any{"call_id": callID})
	if request.Model != "" {
		base["model"] = request.Model
	}
	if request.Provider != "" {
		base["provider"] = request.Provider
	}
	if request.Temperature != nil {
		base["temperature"] = *request.Temperature
	}
	if request.MaxTokens != nil {
		base["max_tokens"] = *request.MaxTokens
	}

	call := &LLMCall{agent: a, base: base, started: time.Now()}
	a.emit(eventLLMStarted, withData(base, nil))
	return call
}

// FirstToken marks when the first token of a streamed answer arrived, which
// the server turns into time to first token. Only the first call counts.
func (c *LLMCall) FirstToken() {
	if !c.hasFirstToken.CompareAndSwap(false, true) {
		return
	}
	c.agent.emit(eventLLMFirstToken, withData(c.base, nil))
}

// End emits llm.completed with usage when err is nil, and llm.failed with the
// error's text otherwise; both carry the call's duration. Only the first End
// counts.
func (c *LLMCall) End(usage Usage, err error) {
	if !c.isEnded.CompareAndSwap(false, true) {
		return
	}
	data := withData(c.base, map[string]any{"duration_ms": since(c.started)})
	if err != nil {
		data["error"] = err.Error()
		c.agent.emit(eventLLMFailed, data)
		return
	}
	putPositive(data, "input_tokens", usage.InputTokens)
	putPositive(data, "output_tokens", usage.OutputTokens)
	putPositive(data, "cached_tokens", usage.CachedTokens)
	if usage.CostUSD > 0 {
		data["cost_usd"] = usage.CostUSD
	}
	putNonEmpty(data, "finish_reason", usage.FinishReason)
	putNonEmpty(data, "response_model", usage.ResponseModel)
	putNonEmpty(data, "response_id", usage.ResponseID)
	c.agent.emit(eventLLMCompleted, data)
}

// ToolCall is a tool call in flight. Its methods are safe for concurrent use.
type ToolCall struct {
	agent   *Agent
	base    map[string]any
	started time.Time
	isEnded atomic.Bool
}

// StartTool emits tool.started for the named tool and returns the call, which
// must be ended with [ToolCall.End]. callID tells apart concurrent calls of
// one tool and is generated when empty; attributes are added to every event
// of the call.
func (a *Agent) StartTool(name, callID string, attributes map[string]any) *ToolCall {
	if callID == "" {
		callID = NewID()
	}
	base := withData(attributes, map[string]any{"tool_name": name, "call_id": callID})

	call := &ToolCall{agent: a, base: base, started: time.Now()}
	a.emit(eventToolStarted, withData(base, nil))
	return call
}

// End emits tool.completed when err is nil and tool.failed with the error's
// text otherwise; both carry the call's duration. Only the first End counts.
func (c *ToolCall) End(err error) {
	if !c.isEnded.CompareAndSwap(false, true) {
		return
	}
	data := withData(c.base, map[string]any{"duration_ms": since(c.started)})
	if err != nil {
		data["error"] = err.Error()
		c.agent.emit(eventToolFailed, data)
		return
	}
	c.agent.emit(eventToolCompleted, data)
}

// since is the time elapsed from start in milliseconds, on the monotonic
// clock.
func since(start time.Time) float64 {
	return float64(time.Since(start)) / float64(time.Millisecond)
}

func putPositive(data map[string]any, key string, value int) {
	if value > 0 {
		data[key] = value
	}
}

func putNonEmpty(data map[string]any, key, value string) {
	if value != "" {
		data[key] = value
	}
}
