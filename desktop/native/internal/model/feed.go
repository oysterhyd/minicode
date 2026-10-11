package model

import (
	"encoding/json"
	"fmt"
	"strings"
)

// Context compaction can remove earlier messages. Match the preserved suffix
// backwards against committed responses, using tool IDs when present.
func reasoningHistory(detail Detail) map[int]Event {
	var responses []Event
	for _, event := range detail.Events {
		if event.Type == "assistant_message" {
			responses = append(responses, event)
		}
	}
	matched := map[int]Event{}
	cursor := len(responses) - 1
	for i := len(detail.Messages) - 1; i >= 0; i-- {
		message := detail.Messages[i]
		if message.Role != "assistant" {
			continue
		}
		var text strings.Builder
		var ids, names []string
		for _, block := range message.Content {
			switch String(block["type"]) {
			case "text":
				text.WriteString(String(block["text"]))
			case "tool_use":
				ids = append(ids, String(block["id"]))
				names = append(names, String(block["name"]))
			}
		}
		for j := cursor; j >= 0; j-- {
			event := responses[j]
			if String(event.Data["text"]) != text.String() {
				continue
			}
			if raw, ok := event.Data["tool_call_ids"]; ok {
				if fmt.Sprint(raw) != fmt.Sprint(ids) {
					continue
				}
			} else if raw, ok := event.Data["tool_calls"]; ok && fmt.Sprint(raw) != fmt.Sprint(names) {
				continue
			}
			matched[i] = event
			cursor = j - 1
			break
		}
	}
	return matched
}

func History(detail Detail) []*FeedItem {
	reasoning := reasoningHistory(detail)
	results := map[string]Event{}
	starts := map[string]Event{}
	decisions := map[string]Event{}
	children := map[string]string{}
	for _, event := range detail.Events {
		id := String(event.Data["call_id"])
		switch event.Type {
		case "tool_call_start":
			starts[id] = event
		case "tool_call_result":
			results[id] = event
		case "approval_decision":
			decisions[id] = event
		case "subagent_start":
			children[String(event.Data["kind"])+":"+String(event.Data["task"])] = String(event.Data["child_session_id"])
		}
	}
	items := make([]*FeedItem, 0, len(detail.Messages))
	for i, message := range detail.Messages {
		var attachments []Attachment
		if message.Role == "user" {
			for _, block := range message.Content {
				if String(block["type"]) == "media" {
					raw, _ := json.Marshal(block)
					var file Attachment
					if json.Unmarshal(raw, &file) == nil {
						attachments = append(attachments, file)
					}
				}
			}
		}
		if event, ok := reasoning[i]; ok && String(event.Data["reasoning"]) != "" {
			items = append(items, &FeedItem{ID: fmt.Sprintf("reasoning-%d", event.Seq), Kind: "reasoning", Text: String(event.Data["reasoning"]), EndedAt: Time(event.Timestamp)})
		}
		for j, block := range message.Content {
			id := fmt.Sprintf("%d-%d", i, j)
			switch String(block["type"]) {
			case "text":
				text := String(block["text"])
				if text != "" {
					items = append(items, &FeedItem{ID: id, Kind: message.Role, Text: text, Attachments: attachments})
					attachments = nil
				}
			case "tool_use":
				callID := String(block["id"])
				args, _ := block["input"].(map[string]any)
				name := String(block["name"])
				if decision, ok := decisions[callID]; ok && !Bool(decision.Data["granted"]) {
					items = append(items, &FeedItem{ID: id + "-approval", Kind: "approval", Name: name, Args: args})
				}
				result, ok := results[callID]
				output := String(result.Data["output_detail"])
				if output == "" {
					output = String(result.Data["error"])
				}
				items = append(items, &FeedItem{ID: callID, Kind: "tool", Name: name, Args: args,
					ChildSessionID: children[String(args["kind"])+":"+String(args["task"])],
					ArtifactID:     String(result.Data["artifact_id"]), Output: output,
					Success: Bool(result.Data["success"]), Pending: false, Interrupted: !ok,
					StartedAt: Time(starts[callID].Timestamp), EndedAt: Time(result.Timestamp)})
			}
		}
		if len(attachments) > 0 {
			items = append(items, &FeedItem{ID: fmt.Sprintf("%d-media", i), Kind: "user", Attachments: attachments})
		}
	}
	return items
}

type FeedRow struct {
	ID       string
	Item     *FeedItem
	Tools    []*FeedItem
	Position int
}

func GroupFeed(items []*FeedItem) []FeedRow {
	rows := make([]FeedRow, 0, len(items))
	for i, item := range items {
		if item.Kind == "tool" {
			if len(rows) > 0 && rows[len(rows)-1].Tools != nil {
				rows[len(rows)-1].Tools = append(rows[len(rows)-1].Tools, item)
			} else {
				rows = append(rows, FeedRow{ID: item.ID, Tools: []*FeedItem{item}, Position: i})
			}
		} else {
			rows = append(rows, FeedRow{ID: item.ID, Item: item, Position: i})
		}
	}
	return rows
}
