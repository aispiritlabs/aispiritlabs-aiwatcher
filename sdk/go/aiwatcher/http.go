package aiwatcher

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"sync/atomic"
	"time"
)

// Defaults for an [HTTPTransport], the same as the Python and TypeScript SDKs'.
const (
	DefaultBatchSize      = 64
	DefaultFlushInterval  = time.Second
	DefaultQueueSize      = 10_000
	DefaultRequestTimeout = 10 * time.Second
)

// ingestPath is the server's HTTP way in for events.
const ingestPath = "/api/v1/events"

// HTTPTransport posts batches of envelopes to POST /api/v1/events.
//
// Send only queues. One goroutine posts a batch once it holds [WithBatchSize]
// envelopes, or [WithFlushInterval] after the first envelope of a batch
// arrived, whichever comes first. The queue is bounded on purpose: telemetry
// must not be able to exhaust the process's memory, so a full queue drops
// events and counts them in [HTTPTransport.Dropped]. A batch the server does
// not accept is dropped, counted and logged, never retried — a producer that
// retries telemetry against a server that is down only moves the outage into
// itself.
//
// Close must be called to stop the goroutine and deliver what is pending.
type HTTPTransport struct {
	url           string
	token         string
	client        *http.Client
	logger        *slog.Logger
	batchSize     int
	flushInterval time.Duration
	queueSize     int
	timeout       time.Duration

	queue   chan Envelope
	closing chan struct{}
	done    chan struct{}
	abort   context.CancelFunc

	// mu guards isClosed against Send: Close takes it exclusively, so once
	// Close holds it no Send is between checking and queueing, and the drain
	// that follows sees every envelope that was queued.
	mu       sync.RWMutex
	isClosed bool
	dropped  atomic.Uint64
}

var _ Transport = (*HTTPTransport)(nil)

// HTTPOption configures an [HTTPTransport].
type HTTPOption func(*HTTPTransport) error

// WithToken sends token as a bearer credential. Needed only against a server
// with single sign-on on; a producer carries a token of its own there, which
// grants the editor role and never admin.
func WithToken(token string) HTTPOption {
	return func(t *HTTPTransport) error {
		t.token = token
		return nil
	}
}

// WithBatchSize sets how many envelopes make a batch that goes out at once.
func WithBatchSize(n int) HTTPOption {
	return func(t *HTTPTransport) error {
		if n < 1 {
			return fmt.Errorf("aiwatcher: batch size %d is below 1", n)
		}
		t.batchSize = n
		return nil
	}
}

// WithFlushInterval sets how long the first envelope of a batch waits for
// the batch to fill before it goes out anyway.
func WithFlushInterval(d time.Duration) HTTPOption {
	return func(t *HTTPTransport) error {
		if d <= 0 {
			return fmt.Errorf("aiwatcher: flush interval %s is not positive", d)
		}
		t.flushInterval = d
		return nil
	}
}

// WithQueueSize bounds how many envelopes may wait to be posted.
func WithQueueSize(n int) HTTPOption {
	return func(t *HTTPTransport) error {
		if n < 1 {
			return fmt.Errorf("aiwatcher: queue size %d is below 1", n)
		}
		t.queueSize = n
		return nil
	}
}

// WithRequestTimeout bounds one post, connecting included.
func WithRequestTimeout(d time.Duration) HTTPOption {
	return func(t *HTTPTransport) error {
		if d <= 0 {
			return fmt.Errorf("aiwatcher: request timeout %s is not positive", d)
		}
		t.timeout = d
		return nil
	}
}

// WithHTTPClient posts through c instead of a client of the transport's own.
func WithHTTPClient(c *http.Client) HTTPOption {
	return func(t *HTTPTransport) error {
		if c == nil {
			return errors.New("aiwatcher: http client is nil")
		}
		t.client = c
		return nil
	}
}

// WithLogger reports dropped batches to l instead of [slog.Default].
func WithLogger(l *slog.Logger) HTTPOption {
	return func(t *HTTPTransport) error {
		if l == nil {
			return errors.New("aiwatcher: logger is nil")
		}
		t.logger = l
		return nil
	}
}

// NewHTTPTransport starts a transport posting to the server at baseURL, such
// as http://aiwatcher-server:8080. It returns an error for a base URL that is
// not absolute http or https, and for an option given a value it refuses.
func NewHTTPTransport(baseURL string, opts ...HTTPOption) (*HTTPTransport, error) {
	base, err := url.Parse(strings.TrimRight(baseURL, "/"))
	if err != nil {
		return nil, fmt.Errorf("aiwatcher: parsing base url: %w", err)
	}
	if base.Scheme != "http" && base.Scheme != "https" || base.Host == "" {
		return nil, fmt.Errorf("aiwatcher: base url %q is not an absolute http or https url", baseURL)
	}

	t := &HTTPTransport{
		url:           base.JoinPath(ingestPath).String(),
		client:        &http.Client{},
		logger:        slog.Default(),
		batchSize:     DefaultBatchSize,
		flushInterval: DefaultFlushInterval,
		queueSize:     DefaultQueueSize,
		timeout:       DefaultRequestTimeout,
		closing:       make(chan struct{}),
		done:          make(chan struct{}),
	}
	for _, opt := range opts {
		if err := opt(t); err != nil {
			return nil, err
		}
	}
	t.queue = make(chan Envelope, t.queueSize)

	// The goroutine's own lifetime: cancelled only when Close runs out of time,
	// which abandons the post in flight and everything still queued.
	ctx, cancel := context.WithCancel(context.Background())
	t.abort = cancel
	go t.run(ctx)
	return t, nil
}

// Send queues events for the next batch without blocking. Events that do not
// fit in the queue, and events sent after Close, are dropped and counted.
func (t *HTTPTransport) Send(events ...Envelope) {
	t.mu.RLock()
	defer t.mu.RUnlock()
	for _, event := range events {
		if t.isClosed {
			t.dropped.Add(1)
			continue
		}
		select {
		case t.queue <- event:
		default:
			t.dropped.Add(1)
		}
	}
}

// Dropped reports how many envelopes never reached the server: refused by a
// full queue or a closed transport, or in a batch that failed.
func (t *HTTPTransport) Dropped() uint64 {
	return t.dropped.Load()
}

// Close posts everything queued and stops the goroutine. When ctx is done
// first, it abandons the post in flight, drops what is left and returns the
// context's error. Calling Close again waits for the first call to finish.
func (t *HTTPTransport) Close(ctx context.Context) error {
	t.mu.Lock()
	if !t.isClosed {
		t.isClosed = true
		close(t.closing)
	}
	t.mu.Unlock()

	select {
	case <-t.done:
		return nil
	case <-ctx.Done():
		t.abort()
		<-t.done
		return fmt.Errorf("aiwatcher: closing http transport: %w", ctx.Err())
	}
}

// run owns the batch: it is the only goroutine that reads the queue.
func (t *HTTPTransport) run(ctx context.Context) {
	defer close(t.done)
	defer t.abort()

	batch := make([]Envelope, 0, t.batchSize)
	// Armed by the first envelope of a batch, stopped when the batch goes out.
	// Since Go 1.23 Stop and Reset need no drain of the channel.
	timer := time.NewTimer(t.flushInterval)
	timer.Stop()
	defer timer.Stop()

	// Batches left once Close ran out of time: counted here and reported in one
	// line, not one line per batch that was never going to be posted.
	abandoned := 0
	flush := func() {
		timer.Stop()
		if len(batch) == 0 {
			return
		}
		if ctx.Err() != nil {
			abandoned += len(batch)
		} else {
			t.post(ctx, batch)
		}
		clear(batch)
		batch = batch[:0]
	}

	for {
		select {
		case event := <-t.queue:
			batch = append(batch, event)
			if len(batch) == 1 {
				timer.Reset(t.flushInterval)
			}
			if len(batch) >= t.batchSize {
				flush()
			}
		case <-timer.C:
			flush()
		case <-t.closing:
			for {
				select {
				case event := <-t.queue:
					batch = append(batch, event)
					if len(batch) >= t.batchSize {
						flush()
					}
				default:
					flush()
					if abandoned > 0 {
						t.drop(abandoned, "closing ran out of time", ctx.Err())
					}
					return
				}
			}
		}
	}
}

// ingestRequest is the body POST /api/v1/events takes.
type ingestRequest struct {
	Events []Envelope `json:"events"`
}

// post sends one batch. Every way it can fail ends the same: the batch is
// counted as dropped and logged once, here, and nothing is returned — there
// is no caller left who could act on it.
func (t *HTTPTransport) post(ctx context.Context, batch []Envelope) {
	body, encoded := t.encode(batch)
	if encoded == 0 {
		return
	}

	ctx, cancel := context.WithTimeout(ctx, t.timeout)
	defer cancel()
	request, err := http.NewRequestWithContext(ctx, http.MethodPost, t.url, bytes.NewReader(body))
	if err != nil {
		t.drop(encoded, "building the request", err)
		return
	}
	request.Header.Set("Content-Type", "application/json")
	if t.token != "" {
		request.Header.Set("Authorization", "Bearer "+t.token)
	}

	response, err := t.client.Do(request)
	if err != nil {
		t.drop(encoded, "posting the batch", err)
		return
	}
	defer response.Body.Close()
	// Read to the end so the connection goes back to the pool; what the body
	// said matters only on a refusal, and only its first few hundred bytes.
	detail, _ := io.ReadAll(io.LimitReader(response.Body, 512))
	_, _ = io.Copy(io.Discard, response.Body)
	if response.StatusCode < 200 || response.StatusCode > 299 {
		t.drop(encoded, "server refused the batch",
			fmt.Errorf("status %d: %s", response.StatusCode, bytes.TrimSpace(detail)))
	}
}

// encode marshals the batch. An envelope whose Data cannot be encoded — a
// channel, a function, a NaN — is dropped on its own rather than taking the
// rest of the batch with it.
func (t *HTTPTransport) encode(batch []Envelope) ([]byte, int) {
	body, err := json.Marshal(ingestRequest{Events: batch})
	if err == nil {
		return body, len(batch)
	}

	kept := make([]Envelope, 0, len(batch))
	for _, event := range batch {
		if _, err := json.Marshal(event); err != nil {
			t.drop(1, "encoding an envelope", err)
			continue
		}
		kept = append(kept, event)
	}
	if len(kept) == 0 {
		return nil, 0
	}
	body, err = json.Marshal(ingestRequest{Events: kept})
	if err != nil {
		t.drop(len(kept), "encoding the batch", err)
		return nil, 0
	}
	return body, len(kept)
}

func (t *HTTPTransport) drop(count int, reason string, err error) {
	t.dropped.Add(uint64(count))
	t.logger.Warn("aiwatcher: dropped events",
		slog.Int("count", count),
		slog.String("reason", reason),
		slog.Any("error", err))
}
