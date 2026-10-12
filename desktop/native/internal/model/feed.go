package model

import (
	"encoding/json"
	"fmt"
	"strings"
)

// Context compaction can remove earlier messages. Match the preserved suffix
// backwards against committed responses, using tool IDs when present.
func reasoningHistory(detail Detail) map[int]Event {
	type key struct {
		text, calls string
		kind        byte
	}
	var responses []Event
	var previous []int
	var latest map[key]int
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
		// Normal histories align directly. Build the recovery index only when
		// compaction or a missing response makes a search necessary.
		if cursor >= 0 && responseMatches(responses[cursor], text.String(), ids, names) {
			matched[i] = responses[cursor]
			cursor--
			continue
		}
		if latest == nil {
			latest = make(map[key]int, len(responses))
			previous = make([]int, len(responses))
			for j, event := range responses {
				k := key{text: String(event.Data["text"])}
				if raw, ok := event.Data["tool_call_ids"]; ok {
					k.kind, k.calls = 1, fmt.Sprint(raw)
				} else if raw, ok := event.Data["tool_calls"]; ok {
					k.kind, k.calls = 2, fmt.Sprint(raw)
				}
				prior, ok := latest[k]
				if !ok {
					prior = -1
				}
				previous[j], latest[k] = prior, j
			}
		}
		// The cursor only moves backwards. Prune each signature's index chain
		// once, so the complete match is linear even with duplicate responses.
		best := -1
		for _, k := range []key{{text: text.String()}, {text.String(), callSignature(ids), 1}, {text.String(), callSignature(names), 2}} {
			j, ok := latest[k]
			if !ok {
				continue
			}
			for j > cursor {
				j = previous[j]
			}
			latest[k] = j
			best = max(best, j)
		}
		if best >= 0 {
			matched[i] = responses[best]
			cursor = best - 1
		}
	}
	return matched
}

func responseMatches(event Event, text string, ids, names []string) bool {
	if String(event.Data["text"]) != text {
		return false
	}
	if raw, ok := event.Data["tool_call_ids"]; ok {
		return fmt.Sprint(raw) == callSignature(ids)
	}
	if raw, ok := event.Data["tool_calls"]; ok {
		return fmt.Sprint(raw) == callSignature(names)
	}
	return true
}

func callSignature(calls []string) string {
	if len(calls) == 0 {
		return "[]"
	}
	return fmt.Sprint(calls)
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
				var images []Attachment
				if Bool(result.Data["success"]) {
					images = ImageReferences(result.Data["images"])
				}
				items = append(items, &FeedItem{ID: callID, Kind: "tool", Name: name, Args: args,
					ChildSessionID: children[String(args["kind"])+":"+String(args["task"])],
					ArtifactID:     String(result.Data["artifact_id"]), Output: output,
					Attachments: images,
					Success:     Bool(result.Data["success"]), Pending: false, Interrupted: !ok,
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

// Only successful tool results expose persisted image references. Do not infer
// reads from filenames: failed calls must never display an unread image.
func ImageReferences(raw any) []Attachment {
	encoded, err := json.Marshal(raw)
	if err != nil {
		return nil
	}
	var files []Attachment
	if json.Unmarshal(encoded, &files) != nil {
		return nil
	}
	var images []Attachment
	for _, file := range files {
		if file.Kind == "image" && len(file.AssetID) == 64 && file.Size > 0 {
			images = append(images, file)
		}
	}
	return images
}

func GroupFeed(items []*FeedItem) []FeedRow {
	rows := make([]FeedRow, 0, len(items))
	for i, item := range items {
		rows = AppendFeed(rows, item, i)
	}
	return rows
}

// AppendFeed updates only the tail. Shared by history loading and live events.
func AppendFeed(rows []FeedRow, item *FeedItem, position int) []FeedRow {
	if item.Kind != "tool" {
		return append(rows, FeedRow{ID: item.ID, Item: item, Position: position})
	}
	if len(rows) > 0 && rows[len(rows)-1].Tools != nil {
		rows[len(rows)-1].Tools = append(rows[len(rows)-1].Tools, item)
		return rows
	}
	return append(rows, FeedRow{ID: item.ID, Tools: []*FeedItem{item}, Position: position})
}
