package model

import (
	"fmt"
	"slices"
	"strings"
)

// AcceptsRunEvent gates incoming bridge run events using the canonical protocol rules
// shared with the Rust bridge and TUI reducer.
func AcceptsRunEvent(
	viewSessionID string,
	abandoned []string,
	viewRunID string,
	awaitingRun bool,
	busy bool,
	eventSessionID string,
	eventRunID string,
	eventName string,
) bool {
	if eventSessionID != "" {
		if slices.Contains(abandoned, eventSessionID) {
			return false
		}
		if viewSessionID != "" && eventSessionID != viewSessionID {
			return false
		}
	}
	if eventName == "bridge_error" || eventRunID == "" {
		return true
	}
	if awaitingRun {
		return eventName == "run_started" && (viewRunID == "" || eventRunID != viewRunID)
	}
	if eventName == "run_started" {
		if busy && viewRunID != "" && eventRunID != viewRunID {
			return false
		}
		return true
	}
	return viewRunID == "" || eventRunID == viewRunID
}

// NoticeForAgentEvent maps canonical runtime agent_event notices into display text and tone.
func NoticeForAgentEvent(eventType string, data map[string]any) (string, string) {
	switch eventType {
	case "context_compacted":
		return "上下文已自动压缩，较早的内容已归档", "info"
	case "goal_check":
		if Bool(data["passed"]) {
			return "验收检查：通过", "info"
		}
		return "验收检查：仍需改进", "info"
	case "background_job_completed":
		return fmt.Sprintf("后台命令 %s：已完成", String(data["job_id"])), "info"
	case "background_job_lost":
		return fmt.Sprintf("后台命令 %s：结果丢失", String(data["job_id"])), "warning"
	case "side_effect_unknown":
		label := String(data["name"])
		if label == "" {
			label = String(data["call_id"])
		}
		return fmt.Sprintf("工具执行结果需复核：%s", label), "warning"
	case "provider_retry":
		nextAttempt := int(Number(data["next_attempt"]))
		if nextAttempt <= 0 {
			nextAttempt = 2
		}
		return fmt.Sprintf(
			"模型请求暂时失败，%g 秒后重试（第 %d 次）。",
			Number(data["delay_s"]),
			nextAttempt,
		), "warning"
	default:
		return "", ""
	}
}

// FormatToolOutput produces canonical tool call output with detail preference and error deduplication.
func FormatToolOutput(data map[string]any) string {
	output := String(data["output_detail"])
	if output == "" {
		output = String(data["output_preview"])
	}
	errText := String(data["error"])
	if errText != "" && !strings.Contains(output, errText) {
		if output == "" {
			return errText
		}
		return output + "\n" + errText
	}
	return output
}
