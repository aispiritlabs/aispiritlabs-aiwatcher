package aiwatcher

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"
	"time"
)

// The archive's way in, and how long one write may take.
const (
	turnsPath           = "/api/v1/conversation-turns"
	DefaultArchiveLimit = 10 * time.Second
)

// Archive records what was said, where a deployment keeps it.
//
// Content never travels on the event log: a turn goes to its own store, sealed,
// under its own retention, and an erasure there actually erases. The log keeps
// the reference — the run, the span and the call this turn belongs to — which
// is what lets a trace show an answer without the words ever reaching it.
//
// Unlike [Client], every method here returns an error and nothing is queued.
// Telemetry must never take a process down, so it is dropped and counted;
// recording what somebody said is the work, and a caller that must not lose it
// has to be told when it was lost.
type Archive struct {
	url       string
	token     string
	client    *http.Client
	timeout   time.Duration
	redactor  Redactor
	consent   Consent
	retention Retention
}

// ArchiveOption configures an [Archive].
type ArchiveOption func(*Archive) error

// WithArchiveToken sends token as a bearer credential. Recording a turn needs
// the editor role; reading one back needs admin, which a producer never has.
func WithArchiveToken(token string) ArchiveOption {
	return func(a *Archive) error {
		a.token = token
		return nil
	}
}

// WithArchiveHTTPClient records through c instead of a client of its own.
func WithArchiveHTTPClient(c *http.Client) ArchiveOption {
	return func(a *Archive) error {
		if c == nil {
			return errors.New("aiwatcher: http client is nil")
		}
		a.client = c
		return nil
	}
}

// WithArchiveTimeout bounds one write, connecting included.
func WithArchiveTimeout(d time.Duration) ArchiveOption {
	return func(a *Archive) error {
		if d <= 0 {
			return fmt.Errorf("aiwatcher: archive timeout %s is not positive", d)
		}
		a.timeout = d
		return nil
	}
}

// WithRedactor runs r over every text part before it leaves the process, and
// records that it ran. A protected deployment refuses a turn with no redaction
// record — not because the claim can be checked, but because it can tell apart
// a producer that has a hook from one that has none.
func WithRedactor(r Redactor) ArchiveOption {
	return func(a *Archive) error {
		if r == nil {
			return errors.New("aiwatcher: redactor is nil")
		}
		a.redactor = r
		return nil
	}
}

// WithConsent sets what permits keeping the content, for turns that name none
// of their own.
func WithConsent(c Consent) ArchiveOption {
	return func(a *Archive) error {
		a.consent = c
		return nil
	}
}

// WithRetention sets how long the content may be held, for turns that ask for
// nothing of their own. A deployment may shorten it, and says so on the turn.
func WithRetention(r Retention) ArchiveOption {
	return func(a *Archive) error {
		if r.TTLDays < 0 {
			return fmt.Errorf("aiwatcher: retention of %d days is not a number of days", r.TTLDays)
		}
		a.retention = r
		return nil
	}
}

// NewArchive returns a client for the archive at baseURL, such as
// http://aiwatcher-server:8080. It refuses a base URL that is not absolute
// http or https, and an option given a value it will not take.
func NewArchive(baseURL string, opts ...ArchiveOption) (*Archive, error) {
	base, err := url.Parse(strings.TrimRight(baseURL, "/"))
	if err != nil {
		return nil, fmt.Errorf("aiwatcher: parsing base url: %w", err)
	}
	if base.Scheme != "http" && base.Scheme != "https" || base.Host == "" {
		return nil, fmt.Errorf("aiwatcher: base url %q is not an absolute http or https url", baseURL)
	}

	archive := &Archive{
		url:       base.JoinPath(turnsPath).String(),
		client:    &http.Client{},
		timeout:   DefaultArchiveLimit,
		redactor:  NoRedaction,
		retention: Retention{TTLDays: 30},
	}
	for _, opt := range opts {
		if err := opt(archive); err != nil {
			return nil, err
		}
	}
	return archive, nil
}

// Role is who spoke.
type Role string

const (
	RoleSystem    Role = "system"
	RoleDeveloper Role = "developer"
	RoleUser      Role = "user"
	RoleAssistant Role = "assistant"
	RoleTool      Role = "tool"
)

// Consent says whose data a turn is, on what basis it is kept, and what that
// permits. An empty Scope permits nothing — an export asking for "train"
// excludes it by name rather than assuming.
type Consent struct {
	// Subject is a pseudonymous handle, never a name: it is what makes an
	// erasure request answerable without the archive holding an identity.
	Subject string
	// Basis is one of unknown, consent, contract, legitimate_interest,
	// synthetic.
	Basis     string
	Reference string
	Scope     []string
	GrantedAt time.Time
}

// Retention is this content's own clock, unrelated to the log's.
type Retention struct {
	TTLDays  int
	PolicyID string
}

// ToolResult is what a tool handed back, kept beside the text rather than
// folded into it: a failure is a signal and a stringified error is not.
type ToolResult struct {
	CallID  string
	Name    string
	OK      bool
	Content string
	Error   string
}

// Provenance ties a turn to the run that produced it. Kept in the clear — it
// is the join to the log, and a trace view reads it to find what was said.
type Provenance struct {
	RunID   string
	TraceID string
	SpanID  string
	AgentID string
	CallID  string
	Model   string
	// Prompt is the registered prompt version this ran on, `name@sha256`.
	Prompt string
}

// Turn is one message, ready to record.
type Turn struct {
	ConversationID  string
	MessageID       string
	ParentMessageID string
	// Ordinal is the turn's place in the conversation, from 0.
	Ordinal     int
	Role        Role
	Text        string
	ToolResults []ToolResult
	Provenance  Provenance
	OccurredAt  time.Time
	// Consent and Retention override the archive's own for this turn.
	Consent   *Consent
	Retention *Retention
}

// Redactor is a producer's own hook, run before anything leaves the process.
// It returns the text to send and the rule ids that fired; unchanged text with
// no rules is a meaningful answer, and a different one from no hook at all.
type Redactor interface {
	Name() string
	Redact(text string) (string, []string)
}

// NoRedaction removes nothing and says so. For content already known to carry
// nobody's data — a fixture, a benchmark — and never a default a caller did
// not choose.
var NoRedaction Redactor = nothingRedacted{}

type nothingRedacted struct{}

func (nothingRedacted) Name() string { return "none" }

func (nothingRedacted) Redact(text string) (string, []string) { return text, nil }

// Record stores turns, replacing any turn already stored under the same
// conversation and message id. It returns an error for a turn the deployment's
// policy refuses, naming every problem it found at once.
func (a *Archive) Record(ctx context.Context, turns ...Turn) error {
	if len(turns) == 0 {
		return nil
	}
	body := make([]map[string]any, 0, len(turns))
	for _, turn := range turns {
		body = append(body, a.encode(turn))
	}
	payload, err := json.Marshal(map[string]any{"turns": body})
	if err != nil {
		return fmt.Errorf("aiwatcher: encoding turns: %w", err)
	}

	ctx, cancel := context.WithTimeout(ctx, a.timeout)
	defer cancel()
	request, err := http.NewRequestWithContext(ctx, http.MethodPost, a.url, bytes.NewReader(payload))
	if err != nil {
		return fmt.Errorf("aiwatcher: building the request: %w", err)
	}
	request.Header.Set("Content-Type", "application/json")
	if a.token != "" {
		request.Header.Set("Authorization", "Bearer "+a.token)
	}

	response, err := a.client.Do(request)
	if err != nil {
		return fmt.Errorf("aiwatcher: recording turns: %w", err)
	}
	defer response.Body.Close()
	detail, _ := io.ReadAll(io.LimitReader(response.Body, 2048))
	_, _ = io.Copy(io.Discard, response.Body)
	if response.StatusCode < 200 || response.StatusCode > 299 {
		return fmt.Errorf("aiwatcher: the archive refused %d turns: status %d: %s",
			len(turns), response.StatusCode, bytes.TrimSpace(detail))
	}
	return nil
}

func (a *Archive) encode(turn Turn) map[string]any {
	text, rules := a.redactor.Redact(turn.Text)
	parts := []map[string]any{}
	if text != "" {
		parts = append(parts, map[string]any{"kind": "text", "text": text})
	}

	results := make([]map[string]any, 0, len(turn.ToolResults))
	for _, result := range turn.ToolResults {
		content, fired := a.redactor.Redact(result.Content)
		rules = append(rules, fired...)
		results = append(results, map[string]any{
			"call_id": result.CallID,
			"name":    result.Name,
			"ok":      result.OK,
			"content": content,
			"error":   result.Error,
		})
	}

	content := map[string]any{"parts": parts}
	if len(results) > 0 {
		content["tool_results"] = results
	}

	body := map[string]any{
		"conversation_id": turn.ConversationID,
		"message_id":      turn.MessageID,
		"ordinal":         turn.Ordinal,
		"role":            string(turn.Role),
		"content":         content,
		"provenance":      provenanceOf(turn.Provenance),
		"policy":          a.policy(turn, unique(rules)),
	}
	if turn.ParentMessageID != "" {
		body["parent_message_id"] = turn.ParentMessageID
	}
	if !turn.OccurredAt.IsZero() {
		body["occurred_at"] = turn.OccurredAt.UTC().Format(time.RFC3339Nano)
	}
	return body
}

func (a *Archive) policy(turn Turn, rules []string) map[string]any {
	consent := a.consent
	if turn.Consent != nil {
		consent = *turn.Consent
	}
	retention := a.retention
	if turn.Retention != nil {
		retention = *turn.Retention
	}

	basis := consent.Basis
	if basis == "" {
		basis = "unknown"
	}
	scope := consent.Scope
	if scope == nil {
		scope = []string{}
	}
	consented := map[string]any{
		"subject":   consent.Subject,
		"basis":     basis,
		"reference": consent.Reference,
		"scope":     scope,
	}
	if !consent.GrantedAt.IsZero() {
		consented["granted_at"] = consent.GrantedAt.UTC().Format(time.RFC3339Nano)
	}

	return map[string]any{
		"consent": consented,
		"retention": map[string]any{
			"ttl_days":  retention.TTLDays,
			"policy_id": retention.PolicyID,
		},
		// The count is what the server cannot see: it holds what is left, not
		// what was taken out.
		"redaction": map[string]any{
			"redactor": a.redactor.Name(),
			"rules":    rules,
			"replaced": len(rules),
		},
	}
}

func provenanceOf(from Provenance) map[string]any {
	body := map[string]any{}
	for key, value := range map[string]string{
		"run_id":   from.RunID,
		"trace_id": from.TraceID,
		"span_id":  from.SpanID,
		"agent_id": from.AgentID,
		"call_id":  from.CallID,
		"model":    from.Model,
		"prompt":   from.Prompt,
	} {
		if value != "" {
			body[key] = value
		}
	}
	return body
}

func unique(rules []string) []string {
	if len(rules) == 0 {
		return []string{}
	}
	seen := make(map[string]struct{}, len(rules))
	out := make([]string, 0, len(rules))
	for _, rule := range rules {
		if _, known := seen[rule]; known {
			continue
		}
		seen[rule] = struct{}{}
		out = append(out, rule)
	}
	return out
}
