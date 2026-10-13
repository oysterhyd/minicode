package workspace

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

func TestResolveContainsPaths(t *testing.T) {
	root := t.TempDir()
	if err := os.WriteFile(filepath.Join(root, "a.txt"), []byte("a"), 0600); err != nil {
		t.Fatal(err)
	}
	if _, err := Resolve(root, "a.txt"); err != nil {
		t.Fatal(err)
	}
	if _, err := Resolve(root, "nested/new.txt"); err != nil {
		t.Fatal(err)
	}
	if _, err := Resolve(root, "../outside.txt"); err == nil {
		t.Fatal("parent path escaped")
	}
	if _, err := Resolve(root, root+"-neighbor/a.txt"); err == nil {
		t.Fatal("prefix path escaped")
	}
}

func auditGit(t *testing.T, root string, args ...string) string {
	t.Helper()
	cmd := exec.Command("git", args...)
	cmd.Dir = root
	data, err := cmd.CombinedOutput()
	if err != nil {
		t.Fatalf("git %v: %s: %v", args, data, err)
	}
	return string(data)
}

func TestSingleFileGitActionsTreatWildcardNamesLiterally(t *testing.T) {
	root := t.TempDir()
	auditGit(t, root, "init", "-q")
	for _, name := range []string{"[ab].txt", "a.txt"} {
		if err := os.WriteFile(filepath.Join(root, name), []byte("before\n"), 0600); err != nil {
			t.Fatal(err)
		}
	}
	auditGit(t, root, "add", "-A")
	auditGit(t, root, "-c", "user.name=Audit", "-c", "user.email=audit@example.invalid", "commit", "-qm", "baseline")
	for _, name := range []string{"[ab].txt", "a.txt"} {
		if err := os.WriteFile(filepath.Join(root, name), []byte("after\n"), 0600); err != nil {
			t.Fatal(err)
		}
	}
	s := New(os.Environ())
	if err := s.Stage(context.Background(), root, "[ab].txt"); err != nil {
		t.Fatal(err)
	}
	staged := auditGit(t, root, "diff", "--cached", "--name-only", "-z")
	if staged != "[ab].txt\x00" {
		t.Fatalf("stage selected extra files: %q", staged)
	}
	diff, err := s.Diff(context.Background(), root, "[ab].txt")
	if err != nil || strings.Contains(diff, "diff --git a/a.txt") {
		t.Fatalf("diff expanded filename: %s: %v", diff, err)
	}
}

func TestNestedWorkspaceStageAllStaysInsideTheWorkspace(t *testing.T) {
	root := t.TempDir()
	auditGit(t, root, "init", "-q")
	sub := filepath.Join(root, "sub")
	if err := os.Mkdir(sub, 0700); err != nil {
		t.Fatal(err)
	}
	for _, name := range []string{"outside.txt", "sub/inside.txt"} {
		if err := os.WriteFile(filepath.Join(root, name), []byte("new\n"), 0600); err != nil {
			t.Fatal(err)
		}
	}
	s := New(os.Environ())
	if err := s.StageAll(context.Background(), sub); err != nil {
		t.Fatal(err)
	}
	staged := auditGit(t, root, "diff", "--cached", "--name-only", "-z")
	if staged != "sub/inside.txt\x00" {
		t.Fatalf("staged outside workspace: %q", staged)
	}
	changes, err := s.Changes(context.Background(), sub)
	if err != nil || len(changes) != 1 || changes[0].Path != "inside.txt" {
		t.Fatalf("nested changes have wrong paths: %+v: %v", changes, err)
	}
}

func TestGitNullParsers(t *testing.T) {
	status := ParseStatus("R  new name.txt\x00old name.txt\x00?? line\nname.txt\x00")
	if len(status) != 2 || status[0].Path != "new name.txt" || status[1].Path != "line\nname.txt" {
		t.Fatalf("%+v", status)
	}
	stats := ParseNumstat("3\t2\t\x00old name.txt\x00new name.txt\x00-\t-\tbinary.bin\x00")
	if stats["new name.txt"].Additions != 3 || stats["new name.txt"].Deletions != 2 {
		t.Fatalf("%+v", stats)
	}
	if CountLines([]byte("a\nb")) != 2 || CountLines([]byte{'a', 0, 'b'}) != 0 {
		t.Fatal("line counter mismatch")
	}
}
