package aiwatcher_test

import (
	"context"
	"errors"
	"fmt"
	"log"

	"github.com/aispiritlabs/aispiritlabs-aiwatcher/sdk/go/aiwatcher"
)

// printing is a transport that prints what it would send.
type printing struct{}

func (printing) Send(events ...aiwatcher.Envelope) {
	for _, event := range events {
		if event.AgentID == "" {
			fmt.Println(*event.Sequence, event.EventType)
			continue
		}
		fmt.Println(*event.Sequence, event.EventType, event.AgentID)
	}
}

func (printing) Close(context.Context) error { return nil }

func ExampleClient_StartRun() {
	client, err := aiwatcher.NewClient("planner-api", aiwatcher.WithTransport(printing{}))
	if err != nil {
		log.Fatal(err)
	}
	defer client.Close(context.Background())

	run := client.StartRun(aiwatcher.NewID(), aiwatcher.RunOptions{ConversationID: "conversation-1"})
	agent := run.StartAgent("assistant")

	call := agent.StartLLM(aiwatcher.LLMRequest{Model: "gemini-flash", Provider: "openrouter"})
	call.End(aiwatcher.Usage{InputTokens: 812, OutputTokens: 193, FinishReason: "stop"}, nil)

	retry := agent.StartLLM(aiwatcher.LLMRequest{Model: "gemini-flash", Provider: "openrouter"})
	err = errors.New("status 429")
	retry.End(aiwatcher.Usage{}, err)

	agent.End(err)
	run.End(err)
	// Output:
	// 0 run.started
	// 1 agent.started assistant
	// 2 llm.started assistant
	// 3 llm.completed assistant
	// 4 llm.started assistant
	// 5 llm.failed assistant
	// 6 agent.failed assistant
	// 7 run.failed
}
