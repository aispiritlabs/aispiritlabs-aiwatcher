package aiwatcher_test

import (
	"context"
	"sync"
	"testing"

	"github.com/aispiritlabs/aispiritlabs-aiwatcher/sdk/go/aiwatcher"
)

// recording is a transport that keeps what it was sent.
type recording struct {
	mu     sync.Mutex
	events []aiwatcher.Envelope
}

var _ aiwatcher.Transport = (*recording)(nil)

func (r *recording) Send(events ...aiwatcher.Envelope) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.events = append(r.events, events...)
}

func (r *recording) Close(context.Context) error { return nil }

func (r *recording) sent() []aiwatcher.Envelope {
	r.mu.Lock()
	defer r.mu.Unlock()
	return append([]aiwatcher.Envelope(nil), r.events...)
}

func (r *recording) last() aiwatcher.Envelope {
	r.mu.Lock()
	defer r.mu.Unlock()
	return r.events[len(r.events)-1]
}

func (r *recording) types() []string {
	var types []string
	for _, event := range r.sent() {
		types = append(types, event.EventType)
	}
	return types
}

func newRecordingClient(t *testing.T) (*aiwatcher.Client, *recording) {
	t.Helper()
	sent := &recording{}
	client, err := aiwatcher.NewClient("planner-api", aiwatcher.WithTransport(sent))
	if err != nil {
		t.Fatalf("NewClient: %v", err)
	}
	return client, sent
}

func TestNewClient_withoutTransportDiscards(t *testing.T) {
	t.Parallel()

	client, err := aiwatcher.NewClient("planner-api")
	if err != nil {
		t.Fatalf("NewClient: %v", err)
	}
	client.StartRun("run-1", aiwatcher.RunOptions{}).End(nil)
	if err := client.Close(t.Context()); err != nil {
		t.Errorf("Close: %v", err)
	}
}
