package main

import (
	"bytes"
	"reflect"
	"strings"
	"testing"
)

func extract(files []sourceFile) (inventory, error) {
	return extractModule(files, "example.org/fixture")
}

func TestImportBindingsUseDeclaredPackageNamesAndExplicitAliases(t *testing.T) {
	files := []sourceFile{
		{"internal/contextjson/decode.go", "package json\ntype Value struct { Enabled bool }; func Unmarshal() {}"},
		{"internal/contextjson/generate.go", "//go:build ignore\npackage main\nfunc main() {}"},
		{"option/options.go", "package option\nimport \"example.org/fixture/internal/contextjson\"\ntype Options struct { Value json.Value }; var Decode = json.Unmarshal"},
		{"option/other.go", "package option\nimport codec \"example.org/fixture/internal/contextjson\"\ntype Other codec.Value"},
	}
	result, err := extract(files)
	if err != nil {
		t.Fatal(err)
	}
	for _, entry := range result.Declarations {
		if entry.Name == "Options" && (entry.Fields[0].Shape.Package != "example.org/fixture/internal/contextjson" || entry.Imports["json"] != "example.org/fixture/internal/contextjson") {
			t.Fatalf("implicit package name lost: %+v", entry)
		}
		if entry.Name == "Other" && entry.Imports["codec"] != "example.org/fixture/internal/contextjson" {
			t.Fatalf("explicit alias lost: %+v", entry)
		}
	}
	for _, fn := range result.Functions {
		if fn.ID == "option#Decode" && fn.Imports["json"] != "example.org/fixture/internal/contextjson" {
			t.Fatalf("decoder alias identity lost: %+v", fn)
		}
	}
	files = append(files, sourceFile{"internal/contextjson/other.go", "package other"})
	if _, err := extract(files); err == nil {
		t.Fatal("ambiguous source package accepted")
	}
}

func TestConfigurationDeclarations(t *testing.T) {
	files := []sourceFile{{"adapter/outbound/options.go", "//go:build windows && !minimal\npackage outbound\nimport opt \"example.org/option\"\ntype Options struct {\n *opt.Shared `json:\",inline\"`\n Timeout int `proxy:\"handshake-timeout,omitempty\"`\n Value *string `yaml:\"value\" json:\"value\"`\n Ignored string `json:\"-\"`\n}\ntype Alias = Options\nfunc (o *Options) UnmarshalJSON(data []byte) error { return nil }\n"}}
	result, err := extract(files)
	if err != nil {
		t.Fatal(err)
	}
	if len(result.Declarations) != 2 || !result.Declarations[0].Alias {
		t.Fatalf("aliases missing: %+v", result.Declarations)
	}
	entry := result.Declarations[1]
	if entry.BuildConstraint != "(windows && !minimal)" || entry.Imports["opt"] != "example.org/option" || !reflect.DeepEqual(entry.Methods, []string{"UnmarshalJSON"}) {
		t.Fatalf("type context missing: %+v", entry)
	}
	if !entry.Fields[0].Embedded || entry.Fields[0].Type != "*opt.Shared" || entry.Fields[1].Tags["proxy"] != "handshake-timeout,omitempty" || entry.Fields[2].Type != "*string" || entry.Fields[3].Tags["json"] != "-" {
		t.Fatalf("field semantics lost: %+v", entry.Fields)
	}
	if entry.Fields[1].Source.Symbol != "Options.Timeout" || entry.Fields[1].Source.Line != 6 {
		t.Fatalf("source evidence incorrect: %+v", entry.Fields[1].Source)
	}
}

func TestTypeShapesPreserveContainersImportsAndGenericArguments(t *testing.T) {
	files := []sourceFile{{"option/types.go", "package option\nimport dep \"example.org/types\"\ntype Options struct { Nested []*dep.Item; Values map[string]*Local; Generic dep.List[Local]; Other interface{} }; type Local struct { Value string }; type Alias Options\n"}}
	inventory, err := extract(files)
	if err != nil {
		t.Fatal(err)
	}
	declarations := map[string]declaration{}
	for _, entry := range inventory.Declarations {
		declarations[entry.Name] = entry
	}
	fields := declarations["Options"].Fields
	nested := fields[0].Shape
	if nested.Kind != "sequence" || nested.Element.Kind != "pointer" || nested.Element.Element.Kind != "named" ||
		nested.Element.Element.Package != "example.org/types" || nested.Element.Element.Name != "Item" {
		t.Fatalf("sequence reference lost: %+v", nested)
	}
	mapping := fields[1].Shape
	if mapping.Kind != "mapping" || mapping.Key.Kind != "builtin" || mapping.Key.Name != "string" ||
		mapping.Element.Element.Package != "option" || mapping.Element.Element.Name != "Local" {
		t.Fatalf("mapping reference lost: %+v", mapping)
	}
	generic := fields[2].Shape
	if generic.Kind != "generic" || generic.Target.Package != "example.org/types" ||
		len(generic.Arguments) != 1 || generic.Arguments[0].Name != "Local" || fields[3].Shape.Kind != "dynamic" {
		t.Fatalf("generic reference lost: %+v", generic)
	}
	if declarations["Alias"].Shape.Kind != "named" || declarations["Alias"].Shape.Name != "Options" {
		t.Fatal("underlying named type was not retained")
	}
}

func TestBehaviorHashIgnoresCommentsAndLayoutButTracksMeaning(t *testing.T) {
	extractFunction := func(body string) function {
		t.Helper()
		result, err := extract([]sourceFile{{"option/value_windows_amd64.go", "//go:build feature || other\npackage option\n" + body}})
		if err != nil || len(result.Functions) != 1 {
			t.Fatalf("function missing: %+v / %v", result, err)
		}
		return result.Functions[0]
	}
	a := extractFunction("func Check() bool { return true }")
	b := extractFunction("// Check describes input.\nfunc Check() bool {\n // An explanation.\n return true\n }")
	c := extractFunction("func Check() bool { return false }")
	if a.BodyHash != b.BodyHash || a.BodyHash == c.BodyHash || len(a.BodyHash) != 64 || a.BuildConstraint != "windows && amd64 && (feature || other)" {
		t.Fatalf("behavior evidence incorrect: %+v / %+v / %+v", a, b, c)
	}
}

func TestCallableAliasesKeepImportIdentityAndMutuallyExclusiveBuildBranches(t *testing.T) {
	files := []sourceFile{
		{"codec/context.go", "//go:build !alternate\npackage codec\nimport json \"example.org/contextjson\"\nvar Decode = json.Unmarshal\n"},
		{"codec/plain.go", "//go:build alternate\npackage codec\nimport json \"encoding/json\"\nfunc Decode(data []byte, value any) error { return json.Unmarshal(data, value) }\n"},
	}
	result, err := extract(files)
	if err != nil || len(result.Functions) != 2 {
		t.Fatalf("missing decoder branches: %+v / %v", result, err)
	}
	for _, entry := range result.Functions {
		if entry.ID != "codec#Decode" || entry.Source.Symbol != "Decode" || len(entry.BodyHash) != 64 {
			t.Fatalf("missing callable identity: %+v", entry)
		}
		if entry.Source.Path == "codec/context.go" {
			if entry.BuildConstraint != "(!alternate)" || entry.Imports["json"] != "example.org/contextjson" {
				t.Fatalf("lost alias binding: %+v", entry)
			}
		} else if entry.BuildConstraint != "(alternate)" || entry.Imports["json"] != "encoding/json" {
			t.Fatalf("lost function binding: %+v", entry)
		}
	}
}

func TestPlatformConstantsBindValueAndBuildBranch(t *testing.T) {
	result, err := extract([]sourceFile{
		{"feature/redirect_linux.go", "package feature\nconst support = true"},
		{"feature/redirect_stub.go", "//go:build !linux\npackage feature\nconst support = false"},
	})
	if err != nil || len(result.Functions) != 2 {
		t.Fatalf("missing conditions: %+v / %v", result, err)
	}
	if result.Functions[0].BodyHash == result.Functions[1].BodyHash || result.Functions[0].BuildConstraint != "linux" || result.Functions[1].BuildConstraint != "(!linux)" {
		t.Fatalf("unbound platform behavior: %+v", result.Functions)
	}
}

func TestSchemaAnnotationsAreSeparateFromWireTags(t *testing.T) {
	result, err := extract([]sourceFile{{"option/value.go", "package option\ntype Options struct { Level string `json:\"level,omitempty\" enum:\"debug,info\" default:\"info\" reference:\"outbound\"` }"}})
	if err != nil {
		t.Fatal(err)
	}
	f := result.Declarations[0].Fields[0]
	if len(f.Tags) != 1 || f.Annotations["enum"] != "debug,info" || f.Annotations["default"] != "info" || f.Annotations["reference"] != "outbound" {
		t.Fatalf("schema annotations missing: %+v", f)
	}
}

func TestBuildDirectivesAndCgoImportBothConstrainDeclarations(t *testing.T) {
	result, err := extract([]sourceFile{{"option/value_windows.go", "// +build feature,amd64\n\npackage option\nimport \"C\"\ntype Options struct{}"}})
	if err != nil {
		t.Fatal(err)
	}
	condition := result.Declarations[0].BuildConstraint
	for _, required := range []string{"windows", "amd64", "feature", "cgo"} {
		if !strings.Contains(condition, required) {
			t.Fatalf("lost build condition %s: %s", required, condition)
		}
	}
}

func TestMethodsAcrossFilesAndDeterministicOrder(t *testing.T) {
	files := []sourceFile{{"option/decode.go", "package option\nfunc (o *Options) UnmarshalYAML(v any) error { return nil }"}, {"option/options.go", "package option\ntype Options struct { A, B int }"}}
	a, err := extract(files)
	if err != nil {
		t.Fatal(err)
	}
	b, err := extract([]sourceFile{files[1], files[0]})
	if err != nil || !reflect.DeepEqual(a, b) || len(a.Declarations[0].Fields) != 2 || len(a.Declarations[0].Methods) != 1 {
		t.Fatalf("non-deterministic or incomplete inventory: %+v / %+v", a, b)
	}
}

func TestRejectsAmbiguousOrInvalidInput(t *testing.T) {
	for _, input := range []string{
		`{"modulePath":"example.org/fixture","files":[]} []`,
		`{"modulePath":"example.org/fixture","files":[{"path":"x.go","content":"package x","unexpected":true}]}`,
		`{"modulePath":"example.org/fixture","files":[{"path":"../x.go","content":"package x"}]}`,
		`{"modulePath":"example.org/fixture","files":[{"path":"x.go","content":"package x"},{"path":"x.go","content":"package x"}]}`,
		`{"modulePath":"example.org/fixture","files":[{"path":"x.go","content":"secret invalid source"}]}`,
		`{"modulePath":"../fixture","files":[]}`,
		`{"files":[]}`,
	} {
		var output bytes.Buffer
		if err := run(strings.NewReader(input), &output); err == nil || strings.Contains(err.Error(), "secret") || output.Len() != 0 {
			t.Fatalf("invalid input not rejected safely: %s", input)
		}
	}
}
