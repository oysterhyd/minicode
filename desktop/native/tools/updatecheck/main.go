// updatecheck exercises MyGo's real updater against a signed release archive
// served on loopback. Its executable must live in a dedicated scratch folder.
package main

import (
	"context"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"sync/atomic"
	"time"

	"github.com/egoist/mygo"
)

func digest(path string) (string, error) {
	f, err := os.Open(path)
	if err != nil {
		return "", err
	}
	defer f.Close()
	h := sha256.New()
	if _, err = io.Copy(h, f); err != nil {
		return "", err
	}
	return hex.EncodeToString(h.Sum(nil)), nil
}

func run() error {
	manifestPath := flag.String("manifest", "", "signed MyGo manifest")
	archivePath := flag.String("archive", "", "signed archive")
	targetPath := flag.String("target", "", "isolated installation directory")
	expectedPath := flag.String("expected", "", "expected new MiniCode.exe")
	port := flag.Int("port", 0, "loopback port compiled into the update feed")
	flag.Parse()
	exe, _ := os.Executable()
	target, err := filepath.Abs(*targetPath)
	if err != nil {
		return err
	}
	if !strings.HasPrefix(filepath.Base(target), "update-qa-") || !strings.EqualFold(filepath.Dir(exe), target) || !strings.EqualFold(filepath.Base(exe), "MiniCode.exe") {
		return fmt.Errorf("updatecheck must run as MiniCode.exe inside its explicit update-qa scratch directory")
	}
	data, err := os.ReadFile(*manifestPath)
	if err != nil {
		return err
	}
	var manifest map[string]any
	if err = json.Unmarshal(data, &manifest); err != nil {
		return err
	}
	listener, err := net.Listen("tcp", fmt.Sprintf("127.0.0.1:%d", *port))
	if err != nil {
		return err
	}
	defer listener.Close()
	manifest["url"] = "http://" + listener.Addr().String() + "/archive"
	delete(manifest, "deltas")
	var corrupt atomic.Bool
	server := &http.Server{Handler: http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/archive" {
			http.ServeFile(w, r, *archivePath)
			return
		}
		copy := map[string]any{}
		for k, v := range manifest {
			copy[k] = v
		}
		if corrupt.Load() {
			sig, _ := base64.StdEncoding.DecodeString(copy["signature"].(string))
			sig[0] ^= 1
			copy["signature"] = base64.StdEncoding.EncodeToString(sig)
		}
		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode(copy)
	})}
	go server.Serve(listener)
	defer server.Close()
	mygo.App.SetName("MiniCode")
	mygo.App.SetVersion("1.1.0")
	if !mygo.Updater.Enabled() {
		return fmt.Errorf("SDK updater was not enabled")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
	defer cancel()
	before, err := digest(exe)
	if err != nil {
		return err
	}
	corrupt.Store(true)
	bad, err := mygo.Updater.Check(ctx)
	if err != nil || bad == nil {
		return fmt.Errorf("check tampered feed: %v", err)
	}
	if err = bad.Install(ctx, nil); err == nil || !strings.Contains(err.Error(), "not signed with the app's key") {
		return fmt.Errorf("tampered update was not rejected by signature: %v", err)
	}
	unchanged, _ := digest(exe)
	if unchanged != before {
		return fmt.Errorf("rejected update changed the executable")
	}
	corrupt.Store(false)
	good, err := mygo.Updater.Check(ctx)
	if err != nil || good == nil {
		return fmt.Errorf("check valid feed: %v", err)
	}
	if err = good.Install(ctx, nil); err != nil {
		return err
	}
	actual, err := digest(filepath.Join(target, "MiniCode.exe"))
	if err != nil {
		return err
	}
	expected, err := digest(*expectedPath)
	if err != nil {
		return err
	}
	if actual != expected {
		return fmt.Errorf("installed executable does not match signed release")
	}
	if _, err = os.Stat(filepath.Join(target, "runtime", "python", "python.exe")); err != nil {
		return err
	}
	mygo.App.SetVersion(good.Version)
	latest, err := mygo.Updater.Check(ctx)
	if err != nil || latest != nil {
		return fmt.Errorf("same version was offered again: %v", err)
	}
	report, _ := json.MarshalIndent(map[string]any{"version": good.Version, "tamper_rejected": true, "installed_sha256": actual, "same_version_ignored": true}, "", "  ")
	if err = os.WriteFile(filepath.Join(target, "update-verification.json"), report, 0600); err != nil {
		return err
	}
	fmt.Println(string(report))
	return nil
}

func main() {
	if err := run(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
