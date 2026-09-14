package aiwatcher

import (
	"context"
	"errors"
	"fmt"
	"maps"
	"sync"
	"time"
)

// MostRunsNumbered is how many runs a client keeps an event count for at
// once. Past it, the run heard from longest ago is forgotten and counts again
// from 0 — the same bound the Python and TypeScript SDKs keep.
const MostRunsNumbered = 100_000

// Client publishes events into runs. It is safe for concurrent use.
//
// A Client numbers the events it sends into each run, from 0, under a client
// id of its own: what lets the server see an event that never reached it,
// whether or not the log numbers its records itself.
type Client struct {
	transport Transport
	source    Source

	mu sync.Mutex
	// sequences holds a count per run, also linked from newest to oldest use
	// so the least recently used one can be forgotten in constant time.
	sequences map[string]*runCount
	newest    *runCount
	oldest    *runCount
}

type runCount struct {
	runID string
	next  uint64
	// newer and older link the counts in order of last use.
	newer, older *runCount
}

// Option configures a [Client].
type Option func(*Client) error

// WithTransport sends events through t. The client takes ownership: closing
// the client closes t. Without this option the client discards everything.
func WithTransport(t Transport) Option {
	return func(c *Client) error {
		if t == nil {
			return errors.New("aiwatcher: transport is nil")
		}
		c.transport = t
		return nil
	}
}

// WithInstance says which replica of the service is publishing.
func WithInstance(instance string) Option {
	return func(c *Client) error {
		c.source.Instance = instance
		return nil
	}
}

// NewClient returns a client publishing as service — the name the explorer's
// runtime pivot groups by. It returns an error for an empty service name and
// for an option given a value it refuses.
func NewClient(service string, opts ...Option) (*Client, error) {
	if service == "" {
		return nil, errors.New("aiwatcher: service name is empty")
	}
	c := &Client{
		transport: discard{},
		source:    Source{Service: service, SDK: sdkName, Client: NewID()},
		sequences: make(map[string]*runCount),
	}
	for _, opt := range opts {
		if err := opt(c); err != nil {
			return nil, err
		}
	}
	return c, nil
}

// Emit publishes one event and returns its id.
//
// It fills in what the client owns — the event id, the contract version, the
// kind, the source and the sequence — and the time when OccurredAt is zero.
// Pass OccurredAt only for something reported after the fact, where stamping
// now would report a duration of zero for work that took minutes. RunID and
// EventType are the caller's; an envelope missing either is still sent, and
// the server refuses it.
//
// Most callers want [Client.StartRun] instead, which emits the events of a
// run, its agents and their calls in the shape the server assembles into a
// trace.
func (c *Client) Emit(event Envelope) string {
	event.SchemaVersion = SchemaVersion
	event.Kind = kindEvent
	event.EventID = NewID()
	event.Source = c.source
	if event.OccurredAt.IsZero() {
		event.OccurredAt = time.Now()
	}
	event.OccurredAt = event.OccurredAt.UTC()
	if event.Data == nil {
		event.Data = map[string]any{}
	}
	sequence := c.nextSequence(event.RunID, event.EventType)
	event.Sequence = &sequence

	c.transport.Send(event)
	return event.EventID
}

// Close flushes and closes the transport. See [Transport.Close].
func (c *Client) Close(ctx context.Context) error {
	if err := c.transport.Close(ctx); err != nil {
		return fmt.Errorf("aiwatcher: closing client: %w", err)
	}
	return nil
}

// nextSequence is this client's count of the events it has sent into one run.
// A run's end forgets its count: nothing more is sent into a finished run.
func (c *Client) nextSequence(runID, eventType string) uint64 {
	c.mu.Lock()
	defer c.mu.Unlock()

	ends := eventType == eventRunCompleted || eventType == eventRunFailed
	count, known := c.sequences[runID]
	if !known {
		if ends {
			return 0
		}
		if len(c.sequences) >= MostRunsNumbered {
			c.forget(c.oldest)
		}
		count = &runCount{runID: runID}
		c.sequences[runID] = count
	} else {
		c.unlink(count)
	}

	sequence := count.next
	if ends {
		delete(c.sequences, runID)
		return sequence
	}
	count.next++
	c.pushNewest(count)
	return sequence
}

func (c *Client) forget(count *runCount) {
	c.unlink(count)
	delete(c.sequences, count.runID)
}

func (c *Client) unlink(count *runCount) {
	if count.newer != nil {
		count.newer.older = count.older
	} else {
		c.newest = count.older
	}
	if count.older != nil {
		count.older.newer = count.newer
	} else {
		c.oldest = count.newer
	}
	count.newer, count.older = nil, nil
}

func (c *Client) pushNewest(count *runCount) {
	count.older = c.newest
	if c.newest != nil {
		c.newest.newer = count
	}
	c.newest = count
	if c.oldest == nil {
		c.oldest = count
	}
}

// withData returns a copy of base with extra's entries added over it, so a
// map a caller passed in is never the one a transport ends up owning.
func withData(base map[string]any, extra map[string]any) map[string]any {
	data := make(map[string]any, len(base)+len(extra))
	maps.Copy(data, base)
	maps.Copy(data, extra)
	return data
}
