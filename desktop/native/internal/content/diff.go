package content

import (
	"regexp"
	"strconv"
	"strings"
)

type DiffLine struct {
	Type    string
	Text    string
	OldLine int
	NewLine int
}

var hunkPattern = regexp.MustCompile(`^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@(.*)$`)

func ParseDiff(text string) []DiffLine {
	lines := make([]DiffLine, 0, strings.Count(text, "\n")+1)
	oldLine, newLine, oldRemaining, newRemaining := 0, 0, 0, 0
	for _, raw := range strings.Split(strings.ReplaceAll(text, "\r\n", "\n"), "\n") {
		if hunk := hunkPattern.FindStringSubmatch(raw); hunk != nil {
			oldLine, _ = strconv.Atoi(hunk[1])
			newLine, _ = strconv.Atoi(hunk[3])
			oldRemaining, newRemaining = 1, 1
			if hunk[2] != "" {
				oldRemaining, _ = strconv.Atoi(hunk[2])
			}
			if hunk[4] != "" {
				newRemaining, _ = strconv.Atoi(hunk[4])
			}
			lines = append(lines, DiffLine{Type: "hunk", Text: raw})
		} else if strings.HasPrefix(raw, "+") && newRemaining > 0 {
			lines = append(lines, DiffLine{Type: "add", Text: raw[1:], NewLine: newLine})
			newLine++
			newRemaining--
		} else if strings.HasPrefix(raw, "-") && oldRemaining > 0 {
			lines = append(lines, DiffLine{Type: "remove", Text: raw[1:], OldLine: oldLine})
			oldLine++
			oldRemaining--
		} else if strings.HasPrefix(raw, " ") && oldRemaining > 0 && newRemaining > 0 {
			lines = append(lines, DiffLine{Type: "context", Text: raw[1:], OldLine: oldLine, NewLine: newLine})
			oldLine++
			newLine++
			oldRemaining--
			newRemaining--
		} else if raw != "" {
			if !strings.HasPrefix(raw, `\`) {
				oldRemaining, newRemaining = 0, 0
			}
			lines = append(lines, DiffLine{Type: "meta", Text: raw})
		}
	}
	return lines
}

func DiffTexts(before, after string) []DiffLine {
	var a, b []string
	if before != "" {
		a = strings.Split(strings.ReplaceAll(before, "\r\n", "\n"), "\n")
	}
	if after != "" {
		b = strings.Split(strings.ReplaceAll(after, "\r\n", "\n"), "\n")
	}
	lines := make([]DiffLine, 0, len(a)+len(b))
	start, aEnd, bEnd := 0, len(a), len(b)
	for start < aEnd && start < bEnd && a[start] == b[start] {
		lines = append(lines, DiffLine{"context", a[start], start + 1, start + 1})
		start++
	}
	for aEnd > start && bEnd > start && a[aEnd-1] == b[bEnd-1] {
		aEnd--
		bEnd--
	}
	var middle []DiffLine
	if start < aEnd && start < bEnd {
		middle = myers(a, b, start, aEnd, bEnd)
	}
	if middle != nil {
		lines = append(lines, middle...)
	} else {
		for i := start; i < aEnd; i++ {
			lines = append(lines, DiffLine{"remove", a[i], i + 1, 0})
		}
		for i := start; i < bEnd; i++ {
			lines = append(lines, DiffLine{"add", b[i], 0, i + 1})
		}
	}
	for i, j := aEnd, bEnd; i < len(a); i, j = i+1, j+1 {
		lines = append(lines, DiffLine{"context", a[i], i + 1, j + 1})
	}
	return lines
}

func myers(a, b []string, start, aEnd, bEnd int) []DiffLine {
	n, m := aEnd-start, bEnd-start
	trace := make([][]int, 0, 16)
	work, cells := 0, 0
	get := func(row []int, k, depth int) int {
		index := k + depth
		if index < 0 || index >= len(row) {
			return -1
		}
		return row[index]
	}
	for depth := 0; depth <= n+m; depth++ {
		cells += 2*depth + 1
		if cells > 250_000 {
			return nil
		}
		row := make([]int, 2*depth+1)
		var previous []int
		if depth > 0 {
			previous = trace[depth-1]
		}
		for k := -depth; k <= depth; k += 2 {
			work++
			if work > 250_000 {
				return nil
			}
			x := 0
			if depth > 0 {
				if k == -depth || (k != depth && get(previous, k-1, depth-1) < get(previous, k+1, depth-1)) {
					x = get(previous, k+1, depth-1)
				} else {
					x = get(previous, k-1, depth-1) + 1
				}
			}
			y := x - k
			for x < n && y < m && a[start+x] == b[start+y] {
				work++
				if work > 250_000 {
					return nil
				}
				x++
				y++
			}
			row[k+depth] = x
			if x >= n && y >= m {
				trace = append(trace, row)
				result := make([]DiffLine, 0, n+m)
				for d := depth; d > 0; d-- {
					k := x - y
					prev := trace[d-1]
					down := k == -d || (k != d && get(prev, k-1, d-1) < get(prev, k+1, d-1))
					prevK := k - 1
					if down {
						prevK = k + 1
					}
					prevX := get(prev, prevK, d-1)
					prevY := prevX - prevK
					for x > prevX && y > prevY {
						result = append(result, DiffLine{"context", a[start+x-1], start + x, start + y})
						x--
						y--
					}
					if down {
						result = append(result, DiffLine{"add", b[start+y-1], 0, start + y})
						y--
					} else {
						result = append(result, DiffLine{"remove", a[start+x-1], start + x, 0})
						x--
					}
				}
				for x > 0 && y > 0 {
					result = append(result, DiffLine{"context", a[start+x-1], start + x, start + y})
					x--
					y--
				}
				for i, j := 0, len(result)-1; i < j; i, j = i+1, j-1 {
					result[i], result[j] = result[j], result[i]
				}
				return result
			}
		}
		trace = append(trace, row)
	}
	return nil
}

func DiffStats(lines []DiffLine) (additions, deletions int) {
	for _, line := range lines {
		if line.Type == "add" {
			additions++
		} else if line.Type == "remove" {
			deletions++
		}
	}
	return
}
