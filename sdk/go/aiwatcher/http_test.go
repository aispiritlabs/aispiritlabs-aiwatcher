package aiwatcher_test

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"io"
	"log/slog"
	"net/http"
	"strings"
	"sync"
	"testing"
	"testing/synctest"
	"time"

	"github.com/aispiritlabs/aispiritlabs-aiwatcher/sdk/go/aiwatcher"
)

// server stands in for aiwatcher behind an http.Client, with no network, so a
// test runs inside a synctest bubble and its clock is the bubble's.
type server struct {
	mu       sync.Mutex
	requests []*http.Request
	batches  [][]aiwatcher.Envelope
	// respond answers a request; nil accepts everything.
	respond func(*http.Request) (*http.Response, error)
}

func (s *server) RoundTrip(request *http.Request) (*http.Response, error) {
	body, err := io.ReadAll(request.Body)
	if err != nil {
		return nil, err
	}
	var ingest struct {
		Events []aiwatcher.Envelope `json:"events"`
	}
	if err := json.Unmarshal(body, &ingest); err != nil {
		return nil, err
	}

	s.mu.Lock()
	s.requests = append(s.requests, request)
	s.batches = append(s.batches, ingest.Events)
	respond := s.respond
	s.mu.Unlock()

	if respond != nil {
		return respond(request)
	}
	return reply(http.StatusAccepted, `{"accepted":1}`), nil
}

func (s *server) sizes() []int {
	s.mu.Lock()
	defer s.mu.Unlock()
	sizes := make([]int, 0, len(s.batches))
	for _, batch := range s.batches {
		sizes = append(sizes, len(batch))
	}
	return sizes
}

func reply(status int, body string) *http.Response {
	return &http.Response{
		StatusCode: status,
		Body:       io.NopCloser(strings.NewReader(body)),
		Header:     make(http.Header),
	}
}

func newTransport(t *testing.T, srv *server, opts ...aiwatcher.HTTPOption) *aiwatcher.HTTPTransport {
	t.Helper()
	opts = append([]aiwatcher.HTTPOption{aiwatcher.WithHTTPClient(&http.Client{Transport: srv})}, opts...)
	transport, err := aiwatcher.NewHTTPTransport("http://aiwatcher-server:8080", opts...)
	if err != nil {
		t.Fatalf("NewHTTPTransport: %v", err)
	}
	return transport
}

func envelopes(n int) []aiwatcher.Envelope {
	events := make([]aiwatcher.Envelope, n)
	for i := range events {
		events[i] = aiwatcher.Envelope{EventType: "custom.noted", RunID: "run-1", Data: map[string]any{"i": i}}
	}
	return events
}

func TestNewHTTPTransport_refusesWhatItCannotUse(t *testing.T) {
	t.Parallel()

	tests := []struct {
		name    string
		baseURL string
		option  aiwatcher.HTTPOption
	}{
		{name: "empty url", baseURL: ""},
		{name: "relative url", baseURL: "aiwatcher-server:8080"},
		{name: "not http", baseURL: "ftp://aiwatcher-server"},
		{name: "no host", baseURL: "http://"},
		{name: "batch below one", option: aiwatcher.WithBatchSize(0)},
		{name: "queue below one", option: aiwatcher.WithQueueSize(0)},
		{name: "flush interval not positive", option: aiwatcher.WithFlushInterval(0)},
		{name: "request timeout not positive", option: aiwatcher.WithRequestTimeout(-time.Second)},
		{name: "nil http client", option: aiwatcher.WithHTTPClient(nil)},
		{name: "nil logger", option: aiwatcher.WithLogger(nil)},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			t.Parallel()

			baseURL, opts := tt.baseURL, []aiwatcher.HTTPOption{}
			if tt.option != nil {
				baseURL, opts = "http://aiwatcher-server:8080", append(opts, tt.option)
			}
			transport, err := aiwatcher.NewHTTPTransport(baseURL, opts...)
			if err == nil {
				_ = transport.Close(t.Context())
				t.Fatal("NewHTTPTransport returned no error")
			}
		})
	}
}

func TestHTTPTransport_Send_postsAFullBatchAtOnce(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		srv := &server{}
		transport := newTransport(t, srv, aiwatcher.WithBatchSize(2), aiwatcher.WithFlushInterval(time.Hour))

		transport.Send(envelopes(3)...)
		synctest.Wait()
		if got := srv.sizes(); len(got) != 1 || got[0] != 2 {
			t.Errorf("posted batches of %v before Close, want one of 2", got)
		}

		if err := transport.Close(t.Context()); err != nil {
			t.Fatalf("Close: %v", err)
		}
		if got := srv.sizes(); len(got) != 2 || got[1] != 1 {
			t.Errorf("posted batches of %v, want the remainder on Close", got)
		}
	})
}

func TestHTTPTransport_Send_postsAPartialBatchAfterTheFlushInterval(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		srv := &server{}
		transport := newTransport(t, srv, aiwatcher.WithFlushInterval(time.Second))
		defer func() {
			if err := transport.Close(t.Context()); err != nil {
				t.Errorf("Close: %v", err)
			}
		}()

		transport.Send(envelopes(1)...)
		synctest.Wait()
		time.Sleep(time.Second - time.Nanosecond)
		synctest.Wait()
		if got := srv.sizes(); len(got) != 0 {
			t.Fatalf("posted %v before the flush interval passed", got)
		}

		transport.Send(envelopes(1)...)
		time.Sleep(time.Nanosecond)
		synctest.Wait()
		if got := srv.sizes(); len(got) != 1 || got[0] != 2 {
			t.Errorf("posted %v, want both events once the first had waited a second", got)
		}
	})
}

func TestHTTPTransport_Send_dropsWhatTheQueueCannotHold(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		release := make(chan struct{})
		srv := &server{respond: func(*http.Request) (*http.Response, error) {
			<-release
			return reply(http.StatusAccepted, ""), nil
		}}
		transport := newTransport(t, srv, aiwatcher.WithBatchSize(1), aiwatcher.WithQueueSize(1))

		transport.Send(envelopes(1)...) // taken off the queue and stuck in a post
		synctest.Wait()
		transport.Send(envelopes(3)...) // one fits the queue, two do not
		if got := transport.Dropped(); got != 2 {
			t.Errorf("Dropped() = %d, want 2", got)
		}

		close(release)
		if err := transport.Close(t.Context()); err != nil {
			t.Fatalf("Close: %v", err)
		}
		if got := srv.sizes(); len(got) != 2 {
			t.Errorf("posted %v, want the stuck batch and the queued one", got)
		}
	})
}

func TestHTTPTransport_Send_afterCloseDrops(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		srv := &server{}
		transport := newTransport(t, srv)
		if err := transport.Close(t.Context()); err != nil {
			t.Fatalf("Close: %v", err)
		}

		transport.Send(envelopes(2)...)
		if got := transport.Dropped(); got != 2 {
			t.Errorf("Dropped() = %d, want 2", got)
		}
		if err := transport.Close(t.Context()); err != nil {
			t.Errorf("a second Close: %v", err)
		}
	})
}

func TestHTTPTransport_Close_givesUpWhenTheContextEnds(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		srv := &server{respond: func(request *http.Request) (*http.Response, error) {
			<-request.Context().Done()
			return nil, request.Context().Err()
		}}
		var logs bytes.Buffer
		transport := newTransport(t, srv,
			aiwatcher.WithBatchSize(1),
			aiwatcher.WithRequestTimeout(time.Hour),
			aiwatcher.WithLogger(slog.New(slog.NewTextHandler(&logs, nil))))

		transport.Send(envelopes(3)...)
		synctest.Wait()
		ctx, cancel := context.WithTimeout(t.Context(), 5*time.Second)
		defer cancel()
		err := transport.Close(ctx)

		if !errors.Is(err, context.DeadlineExceeded) {
			t.Errorf("Close = %v, want the context's deadline", err)
		}
		if got := transport.Dropped(); got != 3 {
			t.Errorf("Dropped() = %d, want the batch in flight and the two never posted", got)
		}
		if got := srv.sizes(); len(got) != 1 {
			t.Errorf("posted %v; batches left after the deadline must not be posted", got)
		}
		if got := strings.Count(logs.String(), "aiwatcher: dropped events"); got != 2 {
			t.Errorf("logged %d drops, want one for the post and one for the rest:\n%s", got, logs.String())
		}
	})
}

func TestHTTPTransport_post(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		srv := &server{}
		transport, err := aiwatcher.NewHTTPTransport("http://aiwatcher.local/behind-a-proxy/",
			aiwatcher.WithHTTPClient(&http.Client{Transport: srv}),
			aiwatcher.WithToken("producer-token"))
		if err != nil {
			t.Fatalf("NewHTTPTransport: %v", err)
		}
		transport.Send(envelopes(1)...)
		if err := transport.Close(t.Context()); err != nil {
			t.Fatalf("Close: %v", err)
		}

		request := srv.requests[0]
		if request.Method != http.MethodPost || request.URL.String() != "http://aiwatcher.local/behind-a-proxy/api/v1/events" {
			t.Errorf("%s %s", request.Method, request.URL)
		}
		if got := request.Header.Get("Authorization"); got != "Bearer producer-token" {
			t.Errorf("Authorization %q", got)
		}
		if got := request.Header.Get("Content-Type"); got != "application/json" {
			t.Errorf("Content-Type %q", got)
		}
	})
}

func TestHTTPTransport_post_dropsARefusedBatch(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		srv := &server{respond: func(*http.Request) (*http.Response, error) {
			return reply(http.StatusUnauthorized, `{"error":"a token is required"}`), nil
		}}
		var logs bytes.Buffer
		transport := newTransport(t, srv, aiwatcher.WithLogger(slog.New(slog.NewTextHandler(&logs, nil))))

		transport.Send(envelopes(2)...)
		if err := transport.Close(t.Context()); err != nil {
			t.Fatalf("Close: %v", err)
		}
		if got := transport.Dropped(); got != 2 {
			t.Errorf("Dropped() = %d, want 2", got)
		}
		if !strings.Contains(logs.String(), "status 401") || !strings.Contains(logs.String(), "a token is required") {
			t.Errorf("the log does not say why:\n%s", logs.String())
		}
		if len(srv.sizes()) != 1 {
			t.Errorf("posted %v; a refused batch is not retried", srv.sizes())
		}
	})
}

func TestHTTPTransport_post_dropsOnlyTheEnvelopeItCannotEncode(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		srv := &server{}
		transport := newTransport(t, srv, aiwatcher.WithLogger(slog.New(slog.DiscardHandler)))

		bad := aiwatcher.Envelope{EventType: "custom.noted", RunID: "run-1", Data: map[string]any{"c": make(chan int)}}
		transport.Send(append(envelopes(1), bad)...)
		if err := transport.Close(t.Context()); err != nil {
			t.Fatalf("Close: %v", err)
		}
		if got := srv.sizes(); len(got) != 1 || got[0] != 1 {
			t.Errorf("posted %v, want the one envelope that encodes", got)
		}
		if got := transport.Dropped(); got != 1 {
			t.Errorf("Dropped() = %d, want 1", got)
		}
	})
}
