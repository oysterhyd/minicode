package model

import (
	"strings"
	"testing"
)

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
