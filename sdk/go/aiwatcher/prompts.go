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

// promptsPath is the registry's way in, and DefaultPromptsLimit how long one
// request to it may take.
const (
	promptsPath         = "/api/v1/prompts"
	DefaultPromptsLimit = 10 * time.Second
)

// Prompts is a client for the prompt registry.
//
// A service that runs on a prompt it keeps in its own source publishes that
// text here on start-up, and names the version on every call it makes. The
// trace then says which prompt an answer came from, and the registry says what
// that prompt was — which is the whole of the join, and the reason the version
// id is the text's own sha256 rather than a number somebody increments.
//
// Publishing is idempotent on the text, so publishing on every start is the
// intended use: the same prompt is the same version, with its original author
// and notes untouched.
//
// Like [Archive] and unlike telemetry, every method returns an error. Reading
// or storing the prompt a service runs on is the work, not a report about it.
type Prompts struct {
	url     string
	token   string
	client  *http.Client
	timeout time.Duration
}

// PromptsOption configures a [Prompts].
type PromptsOption func(*Prompts) error

// WithPromptsToken sends token as a bearer credential. Publishing needs the
// editor role.
func WithPromptsToken(token string) PromptsOption {
	return func(p *Prompts) error {
		p.token = token
		return nil
	}
}

// WithPromptsHTTPClient publishes through c instead of a client of its own.
func WithPromptsHTTPClient(c *http.Client) PromptsOption {
	return func(p *Prompts) error {
		if c == nil {
			return errors.New("aiwatcher: http client is nil")
		}
		p.client = c
		return nil
	}
}

// WithPromptsTimeout bounds one request, connecting included.
func WithPromptsTimeout(d time.Duration) PromptsOption {
	return func(p *Prompts) error {
		if d <= 0 {
			return fmt.Errorf("aiwatcher: prompts timeout %s is not positive", d)
		}
		p.timeout = d
		return nil
	}
}

// NewPrompts returns a registry client for the server at baseURL.
func NewPrompts(baseURL string, opts ...PromptsOption) (*Prompts, error) {
	base, err := url.Parse(strings.TrimRight(baseURL, "/"))
	if err != nil {
		return nil, fmt.Errorf("aiwatcher: parsing base url: %w", err)
	}
	if base.Scheme != "http" && base.Scheme != "https" || base.Host == "" {
		return nil, fmt.Errorf("aiwatcher: base url %q is not an absolute http or https url", baseURL)
	}

	prompts := &Prompts{
		url:     base.JoinPath(promptsPath).String(),
		client:  &http.Client{},
		timeout: DefaultPromptsLimit,
	}
	for _, opt := range opts {
		if err := opt(prompts); err != nil {
			return nil, err
		}
	}
	return prompts, nil
}

// Prompt is a version to store.
type Prompt struct {
	// Name is how the registry is keyed — `planner.assistant`.
	Name string
	// Text is the template, with its variables unfilled: what is compared
	// between versions, and what the version id is the digest of.
	Text string
	// Author, Notes and Model are kept with the first publish of a text and
	// ignored on a re-publish of the same one.
	Author string
	Notes  string
	Model  string
	// Description and Tags replace the prompt's own when given.
	Description string
	Tags        []string
	// Label moves a label — `production` — onto this version. Left empty the
	// version is stored and nothing is deployed, which are different decisions.
	Label    string
	Metadata map[string]string
}

// Published is what a publish did.
type Published struct {
	Name      string
	VersionID string
	// Created is false when this exact text was already stored: a re-run of a
	// deploy is not a new version.
	Created bool
}

// Publish stores a version of a prompt.
func (p *Prompts) Publish(ctx context.Context, prompt Prompt) (Published, error) {
	if strings.TrimSpace(prompt.Name) == "" || prompt.Text == "" {
		return Published{}, errors.New("aiwatcher: a prompt needs a name and its text")
	}
	body := map[string]any{"name": prompt.Name, "text": prompt.Text}
	for key, value := range map[string]string{
		"author":      prompt.Author,
		"notes":       prompt.Notes,
		"model":       prompt.Model,
		"description": prompt.Description,
		"label":       prompt.Label,
	} {
		if value != "" {
			body[key] = value
		}
	}
	if prompt.Tags != nil {
		body["tags"] = prompt.Tags
	}
	if len(prompt.Metadata) > 0 {
		body["metadata"] = prompt.Metadata
	}

	payload, err := json.Marshal(body)
	if err != nil {
		return Published{}, fmt.Errorf("aiwatcher: encoding the prompt: %w", err)
	}

	ctx, cancel := context.WithTimeout(ctx, p.timeout)
	defer cancel()
	request, err := http.NewRequestWithContext(ctx, http.MethodPost, p.url, bytes.NewReader(payload))
	if err != nil {
		return Published{}, fmt.Errorf("aiwatcher: building the request: %w", err)
	}
	request.Header.Set("Content-Type", "application/json")
	if p.token != "" {
		request.Header.Set("Authorization", "Bearer "+p.token)
	}

	response, err := p.client.Do(request)
	if err != nil {
		return Published{}, fmt.Errorf("aiwatcher: publishing the prompt: %w", err)
	}
	defer response.Body.Close()
	answer, _ := io.ReadAll(io.LimitReader(response.Body, 64<<10))
	_, _ = io.Copy(io.Discard, response.Body)
	if response.StatusCode < 200 || response.StatusCode > 299 {
		return Published{}, fmt.Errorf("aiwatcher: the registry refused the prompt: status %d: %s",
			response.StatusCode, bytes.TrimSpace(answer))
	}

	var stored struct {
		Version struct {
			Name      string `json:"name"`
			VersionID string `json:"version_id"`
		} `json:"version"`
		Created bool `json:"created"`
	}
	if err := json.Unmarshal(answer, &stored); err != nil {
		return Published{}, fmt.Errorf("aiwatcher: reading what the registry stored: %w", err)
	}
	return Published{
		Name:      stored.Version.Name,
		VersionID: stored.Version.VersionID,
		Created:   stored.Created,
	}, nil
}
