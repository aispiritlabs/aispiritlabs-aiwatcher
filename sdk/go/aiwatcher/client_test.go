package aiwatcher_test

import (
	"slices"
	"strconv"
	"sync"
	"testing"
	"time"

	"github.com/aispiritlabs/aispiritlabs-aiwatcher/sdk/go/aiwatcher"
)

func TestWithTransport_refusesNil(t *testing.T) {
	t.Parallel()

	if _, err := aiwatcher.NewClient("planner-api", aiwatcher.WithTransport(nil)); err == nil {
		t.Error("NewClient(WithTransport(nil)) returned no error")
	}
}

func TestNewClient_refusesEmptyService(t *testing.T) {
	t.Parallel()

	if _, err := aiwatcher.NewClient(""); err == nil {
		t.Error(`NewClient("") returned no error`)
	}
}

func TestClient_Emit(t *testing.T) {
	t.Parallel()

	sent := &recording{}
	client, err := aiwatcher.NewClient("planner-api",
		aiwatcher.WithTransport(sent), aiwatcher.WithInstance("planner-api-7d9"))
	if err != nil {
		t.Fatalf("NewClient: %v", err)
	}
	reported := time.Date(2026, 9, 14, 12, 0, 0, 0, time.FixedZone("CEST", 2*60*60))

	id := client.Emit(aiwatcher.Envelope{EventType: "custom.noted", RunID: "run-1"})
	client.Emit(aiwatcher.Envelope{EventType: "custom.noted", RunID: "run-1", OccurredAt: reported})

	events := sent.sent()
	first := events[0]
	if first.EventID != id || !uuidV7.MatchString(id) {
		t.Errorf("event id %q, returned %q; want the same version 7 uuid", first.EventID, id)
	}
	if first.SchemaVersion != aiwatcher.SchemaVersion || first.Kind != "Event" {
		t.Errorf("schema version %d, kind %q", first.SchemaVersion, first.Kind)
	}
	expectedSource := aiwatcher.Source{
		Service: "planner-api", Instance: "planner-api-7d9", SDK: "go", Client: first.Source.Client,
	}
	if first.Source != expectedSource || !uuidV7.MatchString(first.Source.Client) {
		t.Errorf("source %+v", first.Source)
	}
	if first.OccurredAt.IsZero() || first.OccurredAt.Location() != time.UTC {
		t.Errorf("occurred at %v, want now in UTC", first.OccurredAt)
	}
	if first.Data == nil {
		t.Error("data is nil; the contract wants an object")
	}
	if !events[1].OccurredAt.Equal(reported) || events[1].OccurredAt.Location() != time.UTC {
		t.Errorf("occurred at %v, want the reported %v in UTC", events[1].OccurredAt, reported)
	}
}

func TestClient_Emit_numbersEachRunFromZero(t *testing.T) {
	t.Parallel()

	client, sent := newRecordingClient(t)
	tracer, tracerSent := newRecordingClient(t)

	run := client.StartRun("run-1", aiwatcher.RunOptions{})
	tracer.Emit(aiwatcher.Envelope{EventType: "custom.noted", RunID: "run-1"})
	run.StartAgent("assistant").End(nil)
	client.StartRun("run-2", aiwatcher.RunOptions{}).End(nil)
	run.End(nil)
	// A finished run's count is forgotten, like the other SDKs forget it.
	client.Emit(aiwatcher.Envelope{EventType: "custom.noted", RunID: "run-1"})

	sequences := map[string][]uint64{}
	for _, event := range sent.sent() {
		sequences[event.RunID] = append(sequences[event.RunID], *event.Sequence)
	}
	expected := map[string][]uint64{"run-1": {0, 1, 2, 3, 0}, "run-2": {0, 1}}
	for runID, want := range expected {
		if got := sequences[runID]; !slices.Equal(got, want) {
			t.Errorf("%s numbered %v, want %v", runID, got, want)
		}
	}
	tracerEvent := tracerSent.sent()[0]
	if *tracerEvent.Sequence != 0 || tracerEvent.Source.Client == sent.sent()[0].Source.Client {
		t.Errorf("a second client in the same run counts on its own under its own id: %+v", tracerEvent)
	}
}

func TestClient_Emit_forgetsTheLeastRecentlyUsedRun(t *testing.T) {
	t.Parallel()

	client, sent := newRecordingClient(t)
	emit := func(runID string) uint64 {
		client.Emit(aiwatcher.Envelope{EventType: "custom.noted", RunID: runID})
		return *sent.last().Sequence
	}

	emit("kept")
	for i := range aiwatcher.MostRunsNumbered - 2 {
		emit("filler-" + strconv.Itoa(i))
	}
	emit("forgotten") // the bound is reached
	emit("kept")      // used again, which leaves filler-0 the least recent
	emit("one-too-many")

	if got := emit("kept"); got != 2 {
		t.Errorf("a recently used run lost its count: sequence %d, want 2", got)
	}
	if got := emit("forgotten"); got != 1 {
		t.Errorf("a run within the bound lost its count: sequence %d, want 1", got)
	}
	if got := emit("filler-0"); got != 0 {
		t.Errorf("the least recently used run kept its count: sequence %d, want 0", got)
	}
}

func TestClient_Emit_isSafeForConcurrentUse(t *testing.T) {
	t.Parallel()

	client, sent := newRecordingClient(t)
	const goroutines, perGoroutine = 8, 50
	var wg sync.WaitGroup
	for range goroutines {
		wg.Go(func() {
			for range perGoroutine {
				client.Emit(aiwatcher.Envelope{EventType: "custom.noted", RunID: "shared"})
			}
		})
	}
	wg.Wait()

	seen := make(map[uint64]bool)
	for _, event := range sent.sent() {
		if seen[*event.Sequence] {
			t.Fatalf("sequence %d given twice", *event.Sequence)
		}
		seen[*event.Sequence] = true
	}
	if len(seen) != goroutines*perGoroutine {
		t.Errorf("%d sequences, want %d", len(seen), goroutines*perGoroutine)
	}
}
