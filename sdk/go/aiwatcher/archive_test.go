package aiwatcher_test

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/aispiritlabs/aispiritlabs-aiwatcher/sdk/go/aiwatcher"
)

// recordingArchive is a server that keeps the one body it was sent.
func recordingArchive(t *testing.T, status int) (*httptest.Server, *[]map[string]any) {
	t.Helper()
	var recorded []map[string]any
	server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		var body struct {
			Turns []map[string]any `json:"turns"`
		}
		if err := json.NewDecoder(request.Body).Decode(&body); err != nil {
			t.Errorf("the archive was sent something that is not json: %v", err)
		}
		recorded = append(recorded, body.Turns...)
		writer.WriteHeader(status)
		if status >= 300 {
			_, _ = writer.Write([]byte(`{"code":"policy_refused","message":"consent.subject is missing"}`))
			return
		}
		_, _ = writer.Write([]byte(`{"turns":[]}`))
	}))
	t.Cleanup(server.Close)
	return server, &recorded
}

type upperRedactor struct{}

func (upperRedactor) Name() string { return "test-redactor@1" }

func (upperRedactor) Redact(text string) (string, []string) {
	if !strings.Contains(text, "@") {
		return text, nil
	}
	return strings.ReplaceAll(text, "ada@example.com", "[redacted]"), []string{"email"}
}

func TestArchive_Record(t *testing.T) {
	t.Parallel()

	server, recorded := recordingArchive(t, http.StatusCreated)
	archive, err := aiwatcher.NewArchive(server.URL,
		aiwatcher.WithRedactor(upperRedactor{}),
		aiwatcher.WithConsent(aiwatcher.Consent{
			Subject: "owner-17", Basis: "contract", Reference: "terms-2026", Scope: []string{"evaluate"},
		}),
		aiwatcher.WithRetention(aiwatcher.Retention{TTLDays: 7, PolicyID: "planner-chat"}))
	if err != nil {
		t.Fatalf("archive: %v", err)
	}

	at := time.Date(2026, 9, 14, 12, 0, 0, 0, time.UTC)
	err = archive.Record(context.Background(), aiwatcher.Turn{
		ConversationID: "conversation:1",
		MessageID:      "m1",
		Ordinal:        0,
		Role:           aiwatcher.RoleUser,
		Text:           "write to ada@example.com about the roof",
		Provenance: aiwatcher.Provenance{
			RunID: "run-1", SpanID: "span-1", AgentID: "planner-assistant", Model: "gemini-flash",
		},
		OccurredAt: at,
	})
	if err != nil {
		t.Fatalf("record: %v", err)
	}

	turns := *recorded
	if len(turns) != 1 {
		t.Fatalf("the archive received %d turns", len(turns))
	}
	turn := turns[0]
	if turn["conversation_id"] != "conversation:1" || turn["role"] != "user" {
		t.Errorf("turn names %v in %v", turn["role"], turn["conversation_id"])
	}

	content, _ := turn["content"].(map[string]any)
	parts, _ := content["parts"].([]any)
	first, _ := parts[0].(map[string]any)
	if first["text"] != "write to [redacted] about the roof" {
		t.Errorf("the hook did not reach the text: %v", first["text"])
	}

	policy, _ := turn["policy"].(map[string]any)
	redaction, _ := policy["redaction"].(map[string]any)
	if redaction["redactor"] != "test-redactor@1" {
		t.Errorf("redaction record %v does not name the hook that ran", redaction)
	}
	rules, _ := redaction["rules"].([]any)
	if len(rules) != 1 || rules[0] != "email" {
		t.Errorf("redaction rules %v", rules)
	}

	consent, _ := policy["consent"].(map[string]any)
	if consent["subject"] != "owner-17" || consent["basis"] != "contract" {
		t.Errorf("consent %v", consent)
	}
	retention, _ := policy["retention"].(map[string]any)
	if retention["ttl_days"] != float64(7) {
		t.Errorf("retention %v", retention)
	}

	provenance, _ := turn["provenance"].(map[string]any)
	if provenance["run_id"] != "run-1" || provenance["span_id"] != "span-1" {
		t.Errorf("provenance %v loses the join to the log", provenance)
	}
	if _, sent := provenance["trace_id"]; sent {
		t.Errorf("provenance %v states a trace id nobody gave it", provenance)
	}
}

func TestArchive_Record_refused(t *testing.T) {
	t.Parallel()

	server, _ := recordingArchive(t, http.StatusUnprocessableEntity)
	archive, err := aiwatcher.NewArchive(server.URL)
	if err != nil {
		t.Fatalf("archive: %v", err)
	}

	err = archive.Record(context.Background(), aiwatcher.Turn{
		ConversationID: "conversation:1", MessageID: "m1", Role: aiwatcher.RoleUser, Text: "hello",
	})
	if err == nil {
		t.Fatal("a refused write reported success; content the caller believes was kept was not")
	}
	if !strings.Contains(err.Error(), "consent.subject is missing") {
		t.Errorf("the error %q does not say what the deployment refused", err)
	}
}

func TestNewArchive_refusesANonAbsoluteURL(t *testing.T) {
	t.Parallel()

	for _, base := range []string{"aiwatcher-server:8080", "", "ftp://host"} {
		if _, err := aiwatcher.NewArchive(base); err == nil {
			t.Errorf("base url %q was accepted", base)
		}
	}
}

func TestArchive_Record_nothingToRecord(t *testing.T) {
	t.Parallel()

	archive, err := aiwatcher.NewArchive("http://localhost:1")
	if err != nil {
		t.Fatalf("archive: %v", err)
	}
	if err := archive.Record(context.Background()); err != nil {
		t.Errorf("recording no turns reached the network: %v", err)
	}
}
