package aiwatcher_test

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/aispiritlabs/aispiritlabs-aiwatcher/sdk/go/aiwatcher"
)

func TestPrompts_Publish(t *testing.T) {
	t.Parallel()

	var sent map[string]any
	server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		if request.Header.Get("Authorization") != "Bearer editor-token" {
			t.Errorf("the registry was asked without the credential: %q", request.Header.Get("Authorization"))
		}
		if err := json.NewDecoder(request.Body).Decode(&sent); err != nil {
			t.Error(err)
		}
		writer.WriteHeader(http.StatusCreated)
		_, _ = writer.Write([]byte(`{"version":{"name":"planner.assistant","version_id":"` + strings.Repeat("a", 64) + `"},"created":true}`))
	}))
	defer server.Close()

	prompts, err := aiwatcher.NewPrompts(server.URL, aiwatcher.WithPromptsToken("editor-token"))
	if err != nil {
		t.Fatalf("prompts: %v", err)
	}

	published, err := prompts.Publish(context.Background(), aiwatcher.Prompt{
		Name:   "planner.assistant",
		Text:   "Jesteś asystentem Plannera.",
		Author: "planner-api",
		Label:  "production",
	})
	if err != nil {
		t.Fatalf("publish: %v", err)
	}
	if published.VersionID != strings.Repeat("a", 64) || !published.Created {
		t.Errorf("published %+v", published)
	}
	if sent["name"] != "planner.assistant" || sent["label"] != "production" {
		t.Errorf("the registry was sent %v", sent)
	}
	// A field the caller left empty is not sent as an empty one: the registry
	// keeps what the first publish of a text said, and a blank would overwrite
	// nothing while looking like an answer.
	if _, present := sent["notes"]; present {
		t.Errorf("an unset field was sent: %v", sent)
	}
}

func TestPrompts_Publish_refused(t *testing.T) {
	t.Parallel()

	server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, _ *http.Request) {
		writer.WriteHeader(http.StatusNotImplemented)
		_, _ = writer.Write([]byte(`{"code":"registry_disabled","message":"AIWATCHER_PROMPT_STORE is none"}`))
	}))
	defer server.Close()

	prompts, err := aiwatcher.NewPrompts(server.URL)
	if err != nil {
		t.Fatalf("prompts: %v", err)
	}

	_, err = prompts.Publish(context.Background(), aiwatcher.Prompt{Name: "planner.assistant", Text: "text"})
	if err == nil {
		t.Fatal("a deployment with no registry reported a stored prompt")
	}
	if !strings.Contains(err.Error(), "AIWATCHER_PROMPT_STORE") {
		t.Errorf("the error %q does not say what is missing", err)
	}
}

func TestPrompts_Publish_needsANameAndText(t *testing.T) {
	t.Parallel()

	prompts, err := aiwatcher.NewPrompts("http://localhost:1")
	if err != nil {
		t.Fatalf("prompts: %v", err)
	}
	for _, prompt := range []aiwatcher.Prompt{{Name: "", Text: "text"}, {Name: "planner.assistant", Text: ""}} {
		if _, err := prompts.Publish(context.Background(), prompt); err == nil {
			t.Errorf("%+v was accepted", prompt)
		}
	}
}
