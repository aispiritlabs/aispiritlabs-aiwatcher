package aiwatcher

import "context"

// Transport carries envelopes to aiwatcher.
//
// Send is called from inside the code being observed, so it must not block on
// I/O: an agent must never wait on its own telemetry. It must be safe for
// concurrent use, and it owns the envelopes it is given — a caller does not
// touch their Data afterwards.
//
// Close delivers what is still pending, or gives up when ctx is done. Events
// sent after Close are dropped.
type Transport interface {
	Send(events ...Envelope)
	Close(ctx context.Context) error
}

// discard is the transport of a client given none.
type discard struct{}

var _ Transport = discard{}

func (discard) Send(...Envelope) {}

func (discard) Close(context.Context) error { return nil }
