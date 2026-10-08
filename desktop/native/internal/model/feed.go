package model

import "fmt"

func History(detail Detail) []*FeedItem {
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
		for j, block := range message.Content {
			id := fmt.Sprintf("%d-%d", i, j)
			switch String(block["type"]) {
			case "text":
				text := String(block["text"])
				if text != "" {
					items = append(items, &FeedItem{ID: id, Kind: message.Role, Text: text})
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
