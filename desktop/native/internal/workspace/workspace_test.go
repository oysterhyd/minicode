package workspace

import (
	"os"
	"path/filepath"
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
