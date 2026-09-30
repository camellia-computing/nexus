package main

import (
	"reflect"
	"strings"
	"testing"
)

func buildReportFixture() []sourceFile {
	return []sourceFile{
		{"main.go", `package main
import "github.com/metacubex/mihomo/constant/features"
import "fmt"
import "strings"
func main() {
 if version {
  if tags := features.Tags(); len(tags) != 0 {
   fmt.Printf("Use tags: %s\n", strings.Join(tags, ", "))
  }
 }
}`},
		{"constant/features/tags.go", `package features
func Tags() (tags []string) {
 if WithFeature { tags = append(tags, "with_feature") }
 return
}`},
		{"constant/features/with_feature.go", "//go:build with_feature\npackage features\nconst WithFeature = true"},
		{"constant/features/with_feature_stub.go", "//go:build !with_feature\npackage features\nconst WithFeature = false"},
	}
}

func TestBuildTagReportBindsBothBranchesAndTheActualVersionOutput(t *testing.T) {
	files := buildReportFixture()
	result, err := extract(files)
	if err != nil {
		t.Fatal(err)
	}
	if len(result.ReportedBuildTags) != 1 || result.ReportedBuildTags[0].Tag != "with_feature" || len(result.ReportedBuildTags[0].Evidence) != 4 {
		t.Fatalf("incomplete tag evidence: %+v", result.ReportedBuildTags)
	}
	for i, j := 0, len(files)-1; i < j; i, j = i+1, j-1 {
		files[i], files[j] = files[j], files[i]
	}
	repeated, err := extract(files)
	if err != nil || !reflect.DeepEqual(repeated, result) {
		t.Fatal("source ordering changed build evidence")
	}
}

func TestPartialOrChangedBuildTagReportsCannotProveAbsence(t *testing.T) {
	for _, change := range []struct {
		index         int
		before, after string
	}{
		{0, "Use tags:", "Other tags:"},
		{0, "len(tags) != 0", "len(tags) > 1"},
		{0, "features.Tags()", "other.Tags()"},
		{1, "WithFeature {", "WithFeature && enabled {"},
		{1, "return", "tags = append(tags, extra); return"},
		{2, "const WithFeature = true", "const WithFeature = enabled"},
		{3, "!with_feature", "!with_feature && linux"},
		{3, "false", "true"},
	} {
		files := buildReportFixture()
		files[change.index].Content = strings.ReplaceAll(files[change.index].Content, change.before, change.after)
		if _, err := extract(files); err == nil {
			t.Fatalf("unproven tag report accepted: %+v", change)
		}
	}
	files := buildReportFixture()
	for _, incomplete := range [][]sourceFile{files[1:], files[:3], append(files, sourceFile{"constant/features/duplicate.go", files[2].Content})} {
		if _, err := extract(incomplete); err == nil {
			t.Fatal("incomplete or ambiguous report accepted")
		}
	}
}
