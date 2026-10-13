package model

import (
	"fmt"
	"math/rand"
	"reflect"
	"strings"
	"testing"
)

func TestHistoryRecoversDeliveredResultsWithoutToolEvents(t *testing.T) {
	detail := Detail{Messages: []Message{
		{Role: "assistant", Content: []map[string]any{{"type": "tool_use", "id": "one", "name": "read"}, {"type": "tool_use", "id": "two", "name": "write"}}},
		{Role: "user", Content: []map[string]any{{"type": "tool_result", "tool_use_id": "one", "content": "persisted failure", "is_error": true}}},
	}}
	items := History(detail)
	if len(items) != 2 || items[0].Output != "persisted failure" || items[0].Success || items[0].Interrupted || !items[1].Interrupted {
		t.Fatalf("history lost persisted result: %+v", items)
	}
}

func TestReasoningIndexPreservesLegacyGreedyMatching(t *testing.T) {
	rng := rand.New(rand.NewSource(19))
	for range 300 {
		var detail Detail
		for i := range 80 {
			data := map[string]any{"text": fmtIntForTest(rng.Intn(4))}
			switch rng.Intn(4) {
			case 0:
				data["tool_call_ids"] = []any{fmtIntForTest(rng.Intn(4))}
			case 1:
				data["tool_calls"] = []any{fmtIntForTest(rng.Intn(4))}
			case 2:
				data["tool_call_ids"] = []any{}
			}
			detail.Events = append(detail.Events, Event{Seq: i, Type: "assistant_message", Data: data})
		}
		for range 60 {
			blocks := []map[string]any{{"type": "text", "text": fmtIntForTest(rng.Intn(6))}}
			if rng.Intn(2) == 0 {
				blocks = append(blocks, map[string]any{"type": "tool_use", "id": fmtIntForTest(rng.Intn(4)), "name": fmtIntForTest(rng.Intn(4))})
			}
			detail.Messages = append(detail.Messages, Message{Role: "assistant", Content: blocks})
		}
		want := map[int]Event{}
		cursor := len(detail.Events) - 1
		for i := len(detail.Messages) - 1; i >= 0; i-- {
			var text strings.Builder
			var ids, names []string
			for _, block := range detail.Messages[i].Content {
				if String(block["type"]) == "text" {
					text.WriteString(String(block["text"]))
				} else {
					ids = append(ids, String(block["id"]))
					names = append(names, String(block["name"]))
				}
			}
			for j := cursor; j >= 0; j-- {
				event := detail.Events[j]
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
				want[i] = event
				cursor = j - 1
				break
			}
		}
		if !reflect.DeepEqual(reasoningHistory(detail), want) {
			t.Fatal("indexed reasoning changed greedy legacy matching")
		}
	}
}

func fmtIntForTest(value int) string { return fmt.Sprint(value) }

func TestReasoningHistoryMatchesPreservedMessages(t *testing.T) {
	detail := Detail{Messages: []Message{
		{Role: "user", Content: []map[string]any{{"type": "text", "text": "summary of compacted context"}}},
		{Role: "assistant", Content: []map[string]any{{"type": "text", "text": "same answer"}}},
		{Role: "assistant", Content: []map[string]any{{"type": "tool_use", "id": "call-new", "name": "read"}}},
	}, Events: []Event{
		{Seq: 1, Type: "assistant_message", Data: map[string]any{"text": "same answer", "reasoning": "old reasoning", "tool_call_ids": []any{}}},
		{Seq: 2, Type: "assistant_message", Data: map[string]any{"text": "same answer", "reasoning": "preserved reasoning", "tool_call_ids": []any{}}},
		{Seq: 3, Type: "assistant_message", Data: map[string]any{"text": "", "reasoning": "tool reasoning", "tool_call_ids": []any{"call-new"}}},
	}}
	items := History(detail)
	if len(items) != 5 || items[1].Kind != "reasoning" || items[1].Text != "preserved reasoning" || items[3].Text != "tool reasoning" || items[4].ID != "call-new" {
		t.Fatalf("reasoning not restored before matching answer/tool: %+v", items)
	}
}

func TestHistoryAssociatesActualImageReadsWithMatchingTool(t *testing.T) {
	asset := strings.Repeat("a", 64)
	file := map[string]any{"asset_id": asset, "kind": "image", "name": "read.png", "mime_type": "image/png", "size": 128}
	detail := Detail{Messages: []Message{{Role: "assistant", Content: []map[string]any{
		{"type": "tool_use", "id": "read-picture", "name": "read", "input": map[string]any{"path": "read.png"}},
		{"type": "tool_use", "id": "read-text", "name": "read", "input": map[string]any{"path": "README.md"}},
	}}}, Events: []Event{
		{Type: "tool_call_result", Data: map[string]any{"call_id": "read-picture", "success": true, "images": []any{file}}},
		{Type: "tool_call_result", Data: map[string]any{"call_id": "read-text", "success": true}},
	}}
	items := History(detail)
	if len(items) != 2 || len(items[0].Attachments) != 1 || items[0].Attachments[0].AssetID != asset || len(items[1].Attachments) != 0 {
		t.Fatal("actual image was not attached to correct tool")
	}
	detail.Events[0].Data["success"] = false
	if images := History(detail)[0].Attachments; len(images) != 0 {
		t.Fatal("failed read displayed an image")
	}
}

func TestHistoryKeepsAttachmentWithUserCaption(t *testing.T) {
	detail := Detail{Messages: []Message{{Role: "user", Content: []map[string]any{{"type": "media", "asset_id": "cached", "kind": "image", "name": "截图.png", "mime_type": "image/png", "size": 512}, {"type": "text", "text": "分析图片"}}}}}
	items := History(detail)
	if len(items) != 1 || items[0].Text != "分析图片" || len(items[0].Attachments) != 1 || items[0].Attachments[0].Name != "截图.png" {
		t.Fatal("restored attachment was detached from its caption")
	}
}
