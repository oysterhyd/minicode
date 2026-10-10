package model

import (
	"encoding/json"
	"fmt"
	"strconv"
	"time"
)

type Session struct {
	ID          string `json:"session_id"`
	Title       string `json:"title"`
	Workspace   string `json:"workspace"`
	Provider    string `json:"provider"`
	Model       string `json:"model"`
	Status      string `json:"status"`
	CreatedAt   string `json:"created_at"`
	UpdatedAt   string `json:"updated_at"`
	Pinned      bool   `json:"pinned"`
	CustomTitle bool   `json:"custom_title"`
}

type Model struct {
	ID             string `json:"id"`
	Name           string `json:"name"`
	ModelID        string `json:"modelId"`
	ServiceID      string `json:"serviceId"`
	Provider       string `json:"provider"`
	Available      bool   `json:"available"`
	SupportsEffort bool   `json:"supportsEffort"`
}

type ServiceModel struct {
	ModelID        string `json:"modelId"`
	Name           string `json:"name"`
	ContextWindow  int    `json:"contextWindow"`
	MaxOutput      int    `json:"maxOutputTokens"`
	SupportsEffort bool   `json:"supportsEffort"`
}

type Service struct {
	ID        string         `json:"id"`
	Name      string         `json:"name"`
	BaseURL   string         `json:"baseUrl"`
	APIStyle  string         `json:"apiStyle"`
	APIKey    string         `json:"apiKey"`
	HasAPIKey bool           `json:"hasApiKey"`
	Enabled   bool           `json:"enabled"`
	Builtin   bool           `json:"builtin"`
	Models    []ServiceModel `json:"models"`
}

type Agent struct {
	OriginalName string   `json:"originalName,omitempty"`
	Name         string   `json:"name"`
	Label        string   `json:"label"`
	Description  string   `json:"description"`
	Instructions string   `json:"instructions"`
	Tools        []string `json:"tools"`
	InheritTools bool     `json:"inheritTools"`
	Builtin      bool     `json:"builtin"`
	Enabled      bool     `json:"enabled"`
}

type Configuration struct {
	Services     []Service `json:"services"`
	Agents       []Agent   `json:"agents"`
	DefaultModel string    `json:"defaultModel"`
}

type Usage struct {
	Input      int  `json:"input_tokens"`
	Output     int  `json:"output_tokens"`
	CacheRead  int  `json:"cache_read_tokens"`
	CacheWrite int  `json:"cache_write_tokens"`
	Available  bool `json:"available"`
}

type UsageSample struct {
	Round             int      `json:"round"`
	Input             int      `json:"input"`
	Output            int      `json:"output"`
	Cached            int      `json:"cached"`
	Available         bool     `json:"available"`
	Seconds           *float64 `json:"seconds"`
	GenerationSeconds *float64 `json:"generationSeconds"`
	FirstTokenSeconds *float64 `json:"firstTokenSeconds"`
	TPS               *float64 `json:"tps"`
}

type Statistics struct {
	ToolCalls    int           `json:"toolCalls"`
	Requests     int           `json:"requests"`
	ModelSeconds float64       `json:"modelSeconds"`
	TPS          *float64      `json:"tps"`
	RequestTPS   *float64      `json:"requestTps"`
	LastTPS      *float64      `json:"lastTps"`
	Samples      []UsageSample `json:"samples"`
}

type Budget struct {
	MaxRounds      int     `json:"max_rounds"`
	MaxTotalTokens int     `json:"max_total_tokens"`
	MaxSeconds     float64 `json:"max_seconds"`
}

type State struct {
	ProtocolVersion int            `json:"protocolVersion"`
	RunID           string         `json:"runId"`
	Running         bool           `json:"running"`
	Phase           string         `json:"phase"`
	Model           string         `json:"model"`
	Effort          string         `json:"effort"`
	PermissionMode  string         `json:"permissionMode"`
	SessionID       string         `json:"sessionId"`
	TaskPending     bool           `json:"taskPending"`
	Rounds          int            `json:"rounds"`
	ContextTokens   int            `json:"contextTokens"`
	ContextWindow   int            `json:"contextWindow"`
	Breakdown       map[string]int `json:"contextBreakdown"`
	Usage           *Usage         `json:"usage"`
	Statistics      *Statistics    `json:"statistics"`
	ActiveModel     string         `json:"activeModel"`
	PendingSettings bool           `json:"pendingSettings"`
	Budget          Budget         `json:"budget"`
	Acceptance      string         `json:"acceptance"`
	AlwaysAllow     []string       `json:"alwaysAllow"`
}

type Command struct {
	Name    string `json:"name"`
	Usage   string `json:"usage"`
	Summary string `json:"summary"`
}

type Skill struct {
	Name        string `json:"name"`
	Description string `json:"description"`
	Origin      string `json:"origin"`
	Active      bool   `json:"active"`
}

type Plugin struct {
	Name    string   `json:"name"`
	Version string   `json:"version"`
	Enabled bool     `json:"enabled"`
	Digest  string   `json:"digest"`
	Path    string   `json:"path"`
	Skills  bool     `json:"skills"`
	Agents  bool     `json:"agents"`
	Servers []string `json:"servers"`
}

type Capabilities struct {
	Skills  []Skill  `json:"skills"`
	Plugins []Plugin `json:"plugins"`
	MCP     []struct {
		Name   string `json:"name"`
		Plugin string `json:"plugin"`
	} `json:"mcp"`
	Agents           []string `json:"agents"`
	AgentDefinitions []Agent  `json:"agentDefinitions"`
}

type Discovery struct {
	Servers []struct {
		Server   string  `json:"server"`
		Protocol *string `json:"protocol"`
		Version  *string `json:"server_version"`
		Error    *string `json:"error"`
	} `json:"servers"`
	Tools []string `json:"tools"`
}

type Change struct {
	Status    string `json:"status"`
	Path      string `json:"path"`
	Additions int    `json:"additions"`
	Deletions int    `json:"deletions"`
}

type Task struct {
	ID     string `json:"task_id"`
	Title  string `json:"title"`
	Status string `json:"status"`
}

type Event struct {
	Seq       int            `json:"seq"`
	Type      string         `json:"type"`
	Timestamp string         `json:"timestamp"`
	Data      map[string]any `json:"data"`
}

type Message struct {
	Role    string           `json:"role"`
	Content []map[string]any `json:"content"`
}

type Detail struct {
	Summary  Session   `json:"summary"`
	Messages []Message `json:"messages"`
	Events   []Event   `json:"events"`
}

type FeedItem struct {
	ID             string
	Kind           string
	Text           string
	Name           string
	Args           map[string]any
	Output         string
	Success        bool
	Pending        bool
	Interrupted    bool
	ApprovalID     string
	Granted        bool
	Deciding       bool
	Auto           bool
	ChildSessionID string
	ArtifactID     string
	StartedAt      time.Time
	EndedAt        time.Time
	Tone           string
	Version        uint64
}

type ApprovalRequest struct {
	ToolName  string         `json:"tool_name"`
	Arguments map[string]any `json:"arguments"`
	Summary   string         `json:"summary"`
}

type RunResult struct {
	ExitReason string `json:"exit_reason"`
	Error      string `json:"error"`
}

type ArtifactPage struct {
	Text       string `json:"text"`
	Content    string `json:"content"`
	Offset     int    `json:"offset"`
	NextOffset *int   `json:"next_offset"`
	Total      int    `json:"total"`
}

func String(value any) string {
	if value == nil {
		return ""
	}
	if s, ok := value.(string); ok {
		return s
	}
	return fmt.Sprint(value)
}

func Number(value any) float64 {
	switch n := value.(type) {
	case float64:
		return n
	case int:
		return float64(n)
	case json.Number:
		f, _ := n.Float64()
		return f
	case string:
		f, _ := strconv.ParseFloat(n, 64)
		return f
	}
	return 0
}

func Bool(value any) bool { b, _ := value.(bool); return b }

func Time(value string) time.Time { t, _ := time.Parse(time.RFC3339Nano, value); return t }
