package workspace

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"sync"
	"time"

	"minicode.desktop/internal/model"
)

const PreviewLimit = 1 << 20
const FileLimit = 5000

type Service struct {
	Environment []string
	slots       chan struct{}
}

func New(environment []string) *Service {
	return &Service{Environment: environment, slots: make(chan struct{}, 4)}
}

func Resolve(root, relative string) (string, error) {
	if root == "" {
		return "", errors.New("请先选择工作区")
	}
	canonical, err := filepath.EvalSymlinks(root)
	if err != nil {
		return "", err
	}
	canonical, err = filepath.Abs(canonical)
	if err != nil {
		return "", err
	}
	target := relative
	if !filepath.IsAbs(target) {
		target = filepath.Join(root, target)
	}
	target, err = filepath.Abs(target)
	if err != nil {
		return "", err
	}
	ancestor := target
	for {
		_, statErr := os.Lstat(ancestor)
		if statErr == nil {
			break
		}
		if !os.IsNotExist(statErr) {
			return "", statErr
		}
		parent := filepath.Dir(ancestor)
		if parent == ancestor {
			return "", errors.New("无法解析文件路径")
		}
		ancestor = parent
	}
	resolved, err := filepath.EvalSymlinks(ancestor)
	if err != nil {
		return "", err
	}
	tail, err := filepath.Rel(ancestor, target)
	if err != nil {
		return "", err
	}
	target = filepath.Join(resolved, tail)
	checkRoot, checkTarget := canonical, target
	if runtime.GOOS == "windows" {
		checkRoot, checkTarget = strings.ToLower(checkRoot), strings.ToLower(checkTarget)
	}
	rel, err := filepath.Rel(checkRoot, checkTarget)
	if err != nil || rel == ".." || strings.HasPrefix(rel, ".."+string(filepath.Separator)) {
		return "", errors.New("文件不在工作区内")
	}
	return target, nil
}

type cappedBuffer struct{ bytes.Buffer }

func (b *cappedBuffer) Write(p []byte) (int, error) {
	if b.Len()+len(p) > 32<<20 {
		return 0, errors.New("Git output exceeds 32 MiB")
	}
	return b.Buffer.Write(p)
}

func (s *Service) Git(ctx context.Context, root string, args ...string) (string, error) {
	select {
	case s.slots <- struct{}{}:
	case <-ctx.Done():
		return "", ctx.Err()
	}
	defer func() { <-s.slots }()
	ctx, cancel := context.WithTimeout(ctx, 30*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, "git", args...)
	cmd.Dir = root
	cmd.Env = s.Environment
	hideWindow(cmd)
	var output, stderr cappedBuffer
	cmd.Stdout, cmd.Stderr = &output, &stderr
	err := cmd.Run()
	if err != nil {
		if ctx.Err() != nil {
			return output.String(), ctx.Err()
		}
		return output.String(), fmt.Errorf("%s: %w", strings.TrimSpace(stderr.String()), err)
	}
	return output.String(), nil
}

func (s *Service) Files(ctx context.Context, root, query string) ([]string, error) {
	if root == "" {
		return []string{}, nil
	}
	output, err := s.Git(ctx, root, "ls-files", "-z", "--cached", "--others", "--exclude-standard")
	files := make([]string, 0)
	seen := make(map[string]bool)
	needle := strings.ToLower(query)
	if err == nil {
		for _, file := range strings.Split(output, "\x00") {
			if file != "" && !seen[file] && strings.Contains(strings.ToLower(file), needle) {
				files = append(files, file)
				seen[file] = true
				if len(files) >= FileLimit {
					break
				}
			}
		}
		return files, nil
	}
	err = filepath.WalkDir(root, func(path string, entry os.DirEntry, walkErr error) error {
		if ctx.Err() != nil {
			return ctx.Err()
		}
		if walkErr != nil {
			return nil
		}
		if len(files) >= FileLimit {
			return filepath.SkipAll
		}
		if path == root {
			return nil
		}
		if entry.IsDir() {
			switch entry.Name() {
			case "node_modules", "dist", "build", "__pycache__":
				return filepath.SkipDir
			}
			if strings.HasPrefix(entry.Name(), ".") {
				return filepath.SkipDir
			}
			return nil
		}
		if entry.Type()&os.ModeSymlink != 0 {
			return nil
		}
		relative, _ := filepath.Rel(root, path)
		relative = filepath.ToSlash(relative)
		if strings.Contains(strings.ToLower(relative), needle) {
			files = append(files, relative)
		}
		return nil
	})
	return files, err
}

func ParseStatus(output string) []model.Change {
	fields := strings.Split(output, "\x00")
	result := make([]model.Change, 0, len(fields))
	for i := 0; i < len(fields); i++ {
		record := fields[i]
		if len(record) < 4 {
			continue
		}
		status := record[:2]
		result = append(result, model.Change{Status: status, Path: record[3:]})
		if strings.ContainsAny(status, "RC") {
			i++
		}
	}
	return result
}

type Stat struct{ Additions, Deletions int }

func ParseNumstat(output string) map[string]Stat {
	fields := strings.Split(output, "\x00")
	result := make(map[string]Stat)
	for i := 0; i < len(fields); i++ {
		record := fields[i]
		parts := strings.SplitN(record, "\t", 3)
		if len(parts) != 3 {
			continue
		}
		path := parts[2]
		if path == "" {
			i += 2
			if i >= len(fields) {
				break
			}
			path = fields[i]
		}
		if path == "" {
			continue
		}
		a, _ := strconv.Atoi(parts[0])
		d, _ := strconv.Atoi(parts[1])
		old := result[path]
		result[path] = Stat{old.Additions + a, old.Deletions + d}
	}
	return result
}

func CountLines(data []byte) int {
	if len(data) == 0 || bytes.IndexByte(data, 0) >= 0 {
		return 0
	}
	count := bytes.Count(data, []byte{'\n'})
	if data[len(data)-1] != '\n' {
		count++
	}
	return count
}

func (s *Service) Changes(ctx context.Context, root string) ([]model.Change, error) {
	if root == "" {
		return []model.Change{}, nil
	}
	// Status and statistics are independent reads and share a bounded worker pool.
	var status, stats string
	var statusErr, statsErr error
	var wg sync.WaitGroup
	wg.Add(2)
	go func() {
		defer wg.Done()
		status, statusErr = s.Git(ctx, root, "-c", "core.quotePath=false", "status", "--porcelain=v1", "-z", "-uall")
	}()
	go func() {
		defer wg.Done()
		stats, statsErr = s.Git(ctx, root, "-c", "core.quotePath=false", "diff", "--numstat", "-z", "--no-ext-diff", "HEAD")
	}()
	wg.Wait()
	if statusErr != nil {
		if strings.Contains(statusErr.Error(), "not a git repository") {
			return []model.Change{}, nil
		}
		return nil, statusErr
	}
	entries := ParseStatus(status)
	totals := ParseNumstat(stats)
	if statsErr != nil && len(entries) > 0 {
		staged, e := s.Git(ctx, root, "diff", "--numstat", "-z", "--no-ext-diff", "--cached")
		if e == nil {
			totals = ParseNumstat(staged)
		}
		unstaged, e := s.Git(ctx, root, "diff", "--numstat", "-z", "--no-ext-diff")
		if e == nil {
			for path, stat := range ParseNumstat(unstaged) {
				old := totals[path]
				totals[path] = Stat{old.Additions + stat.Additions, old.Deletions + stat.Deletions}
			}
		}
	}
	for i := range entries {
		entry := &entries[i]
		if stat, ok := totals[entry.Path]; ok {
			entry.Additions, entry.Deletions = stat.Additions, stat.Deletions
		} else if entry.Status == "??" {
			path, e := Resolve(root, entry.Path)
			if e != nil {
				continue
			}
			info, e := os.Stat(path)
			if e == nil && info.Mode().IsRegular() && info.Size() <= PreviewLimit {
				data, _ := os.ReadFile(path)
				entry.Additions = CountLines(data)
			}
		}
	}
	return entries, nil
}

func Read(root, relative string) (string, bool, error) {
	path, err := Resolve(root, relative)
	if err != nil {
		return "", false, err
	}
	file, err := os.Open(path)
	if err != nil {
		return "", false, err
	}
	defer file.Close()
	info, err := file.Stat()
	if err != nil {
		return "", false, err
	}
	if !info.Mode().IsRegular() {
		return "", false, errors.New("路径不是普通文件")
	}
	data, err := io.ReadAll(io.LimitReader(file, PreviewLimit))
	return strings.ToValidUTF8(string(data), "\uFFFD"), info.Size() > int64(len(data)), err
}

func (s *Service) Diff(ctx context.Context, root, relative string) (string, error) {
	path, err := Resolve(root, relative)
	if err != nil {
		return "", err
	}
	output, err := s.Git(ctx, root, "diff", "HEAD", "--no-ext-diff", "--", relative)
	unborn := false
	if err != nil {
		_, headErr := s.Git(ctx, root, "rev-parse", "--verify", "--quiet", "HEAD")
		ref, refErr := s.Git(ctx, root, "symbolic-ref", "-q", "HEAD")
		unborn = headErr != nil && refErr == nil && strings.HasPrefix(strings.TrimSpace(ref), "refs/heads/")
		if !unborn {
			return "", err
		}
	}
	if strings.TrimSpace(output) != "" {
		if len(output) > 200_000 {
			output = output[:200_000] + "\n...[差异预览已截断，请用 Git 或 Agent 工具查看完整差异]"
		}
		return strings.ToValidUTF8(output, "\uFFFD"), nil
	}
	if _, err := os.Stat(path); err == nil {
		_, trackedErr := s.Git(ctx, root, "ls-files", "--error-unmatch", "--", relative)
		if unborn || trackedErr != nil {
			text, truncated, err := Read(root, relative)
			if err != nil {
				return "", err
			}
			if truncated {
				return "文件超过 1 MiB，差异预览已省略。可用 Agent 的 read 工具分页查看。", nil
			}
			lines := strings.Split(strings.TrimSuffix(strings.ReplaceAll(text, "\r\n", "\n"), "\n"), "\n")
			if text == "" {
				lines = nil
			}
			var out strings.Builder
			fmt.Fprintf(&out, "--- /dev/null\n+++ b/%s\n@@ -0,0 +1,%d @@\n", relative, len(lines))
			for _, line := range lines {
				out.WriteByte('+')
				out.WriteString(line)
				out.WriteByte('\n')
			}
			return out.String(), nil
		}
	}
	return "暂无未提交差异", nil
}

func (s *Service) Stage(ctx context.Context, root, relative string) error {
	if _, err := Resolve(root, relative); err != nil {
		return err
	}
	_, err := s.Git(ctx, root, "add", "--", relative)
	return err
}

func (s *Service) StageAll(ctx context.Context, root string) error {
	if _, err := Resolve(root, "."); err != nil {
		return err
	}
	_, err := s.Git(ctx, root, "add", "-A")
	return err
}

func (s *Service) Unstage(ctx context.Context, root, relative string) error {
	if _, err := Resolve(root, relative); err != nil {
		return err
	}
	if _, err := s.Git(ctx, root, "restore", "--staged", "--", relative); err == nil {
		return nil
	}
	_, headErr := s.Git(ctx, root, "rev-parse", "--verify", "--quiet", "HEAD")
	if headErr != nil {
		// A repository without its first commit has no HEAD to restore from.
		_, err := s.Git(ctx, root, "rm", "--cached", "--", relative)
		return err
	}
	_, err := s.Git(ctx, root, "reset", "-q", "HEAD", "--", relative)
	return err
}
