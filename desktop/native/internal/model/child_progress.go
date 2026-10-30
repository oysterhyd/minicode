package model

import (
	"fmt"
	"sort"
)

type ChildStep struct {
	ID, Name, Status string
	FirstSeq, Seq    int
}

// ApplyChildEvent is shared by live updates and persisted history. Only summary
// fields are consumed; child arguments, output and reasoning never enter the feed.
func ApplyChildEvent(items []*FeedItem, event Event) {
	if event.Type != "subagent_start" && event.Type != "subagent_progress" && event.Type != "subagent_result" {
		return
	}
	data := event.Data
	call, child := String(data["call_id"]), String(data["child_session_id"])
	for i := len(items) - 1; i >= 0; i-- {
		item := items[i]
		if item.Kind != "tool" || item.Name != "delegate" {
			continue
		}
		match := call != "" && item.ID == call
		if call == "" {
			match = child != "" && item.ChildSessionID == child || event.Type == "subagent_start" && item.Pending && String(item.Args["kind"]) == String(data["kind"]) && String(item.Args["task"]) == String(data["task"])
		}
		if !match {
			continue
		}
		if child != "" && item.ChildSessionID != "" && item.ChildSessionID != child {
			item.ChildProgress = nil
			item.ChildSeq = 0
			item.ChildStatus = ""
			item.ChildFinished = false
		}
		if child != "" {
			item.ChildSessionID = child
		}
		switch event.Type {
		case "subagent_result":
			item.ChildStatus = String(data["exit_reason"])
			item.ChildFinished = true
			for j := range item.ChildProgress {
				if item.ChildProgress[j].Status == "running" {
					item.ChildProgress[j].Status = "interrupted"
				}
			}
		case "subagent_progress":
			seq := int(Number(data["child_event_seq"]))
			if seq < 0 {
				return
			}
			if !item.ChildFinished && seq > item.ChildSeq {
				item.ChildSeq = seq
				item.ChildStatus = String(data["name"])
				if String(data["phase"]) == "model" {
					item.ChildStatus = "model"
				}
			}
			if id := String(data["child_call_id"]); String(data["phase"]) == "tool" && id != "" {
				at := -1
				for j, step := range item.ChildProgress {
					if step.ID == id {
						at = j
						break
					}
				}
				next := ChildStep{ID: id, Name: String(data["name"]), Status: String(data["status"]), FirstSeq: seq, Seq: seq}
				if item.ChildFinished && next.Status == "running" {
					next.Status = "interrupted"
				}
				if at < 0 {
					item.ChildProgress = append(item.ChildProgress, next)
				} else {
					old := &item.ChildProgress[at]
					next.FirstSeq = min(seq, old.FirstSeq)
					if seq > old.Seq {
						*old = next
					} else {
						old.FirstSeq = next.FirstSeq
					}
				}
				sort.SliceStable(item.ChildProgress, func(a, b int) bool { return item.ChildProgress[a].FirstSeq < item.ChildProgress[b].FirstSeq })
			}
		}
		item.Version++
		return
	}
}

func ChildStatusLabel(status string) string {
	switch status {
	case "":
		return "准备中"
	case "model":
		return "思考中"
	case "completed":
		return "已完成"
	case "failed", "error", "internal_error":
		return "失败"
	case "provider_error":
		return "模型请求失败"
	case "cancelled":
		return "已取消"
	case "interrupted":
		return "已中断"
	case "running":
		return "运行中"
	case "max_rounds", "time_budget", "token_budget", "context_limit":
		return "已暂停"
	default:
		return status
	}
}

func ChildSummary(item *FeedItem) string {
	return fmt.Sprintf("子任务：%s · %d 次工具调用", ChildStatusLabel(item.ChildStatus), len(item.ChildProgress))
}
