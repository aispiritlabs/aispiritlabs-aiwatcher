package aiwatcher

import (
	"crypto/rand"
	"encoding/binary"
	"encoding/hex"
	"time"
)

// SchemaVersion is the envelope contract this package writes. The server
// rejects an envelope from a newer contract rather than half-reading it.
const SchemaVersion = 1

// CallerRunHeader is the HTTP header a request to a model server carries to
// name the run whose model call it is, so the server can report that run as
// the one it served.
const CallerRunHeader = "Aiwatcher-Caller-Run"

// sdkName is what [Source.SDK] says for everything this package publishes.
const sdkName = "go"

// kindEvent is the only envelope kind the server ingests today.
const kindEvent = "Event"

// Source says which producer sent an event.
type Source struct {
	// Service is the producing service: the explorer's runtime pivot.
	Service string `json:"service"`
	// Instance tells apart replicas of one service.
	Instance string `json:"instance,omitempty"`
	// SDK names the library that built the envelope.
	SDK string `json:"sdk"`
	// Client is the one client that sent the event, which its sequence counts
	// under: a process may hold two clients publishing into one run.
	Client string `json:"client,omitempty"`
}

// Envelope is one event on the wire, as contracts/envelope.schema.json
// describes it. Optional fields left empty are omitted, and the server derives
// the trace and span ids from the run, the agent and data.call_id.
//
// [Client.Emit] fills in the id, the time, the sequence, the source and the
// contract version; a producer building an Envelope by hand fills in only what
// it is reporting.
type Envelope struct {
	SchemaVersion  int       `json:"schema_version"`
	Kind           string    `json:"kind"`
	EventID        string    `json:"event_id"`
	EventType      string    `json:"event_type"`
	OccurredAt     time.Time `json:"occurred_at"`
	RunID          string    `json:"run_id"`
	ConversationID string    `json:"conversation_id,omitempty"`
	WorkflowID     string    `json:"workflow_id,omitempty"`
	WorkflowRunID  string    `json:"workflow_run_id,omitempty"`
	AgentID        string    `json:"agent_id,omitempty"`
	VariantID      string    `json:"variant_id,omitempty"`
	// Sequence is the sending client's count of the events it sent into the
	// run, from 0. A pointer because 0 is a count and absent is not.
	Sequence      *uint64 `json:"sequence,omitempty"`
	TraceID       string  `json:"trace_id,omitempty"`
	SpanID        string  `json:"span_id,omitempty"`
	ParentSpanID  string  `json:"parent_span_id,omitempty"`
	CorrelationID string  `json:"correlation_id,omitempty"`
	CausationID   string  `json:"causation_id,omitempty"`
	Source        Source  `json:"source"`
	// Data is the type-specific payload; docs/event-catalog.md lists the
	// fields the server reads. It is owned by the transport once sent.
	Data map[string]any `json:"data"`
}

// NewID returns a random UUIDv7: time-ordered, so ids minted in one process
// sort by when they were minted, which is what the envelope contract asks of
// an event id. Also a sound run id when the caller has none of its own.
func NewID() string {
	var id [16]byte
	binary.BigEndian.PutUint64(id[:8], uint64(time.Now().UnixMilli())<<16)
	// crypto/rand.Read never returns an error since Go 1.24; it aborts the
	// program instead of handing out predictable bytes.
	_, _ = rand.Read(id[6:])
	id[6] = id[6]&0x0f | 0x70 // version 7
	id[8] = id[8]&0x3f | 0x80 // RFC 9562 variant

	var text [36]byte
	hex.Encode(text[0:8], id[0:4])
	text[8] = '-'
	hex.Encode(text[9:13], id[4:6])
	text[13] = '-'
	hex.Encode(text[14:18], id[6:8])
	text[18] = '-'
	hex.Encode(text[19:23], id[8:10])
	text[23] = '-'
	hex.Encode(text[24:36], id[10:16])
	return string(text[:])
}
