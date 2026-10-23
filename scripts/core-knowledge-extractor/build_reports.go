package main

import (
	"fmt"
	"go/ast"
	"go/build/constraint"
	"go/parser"
	"go/token"
	"sort"
	"strings"
)

type reportedBuildTag struct {
	Tag      string             `json:"tag"`
	Evidence []registryEvidence `json:"evidence"`
}

// Each tag describes a source-proven complete report of one compiler condition,
// not a claim that the report includes all possible compiler flags.
func extractReportedBuildTags(files []sourceFile, packageNames map[string]string) ([]reportedBuildTag, error) {
	parsed := map[string]registryFile{}
	for _, source := range files {
		if source.Path != "main.go" && !strings.HasPrefix(source.Path, "constant/features/") {
			continue
		}
		set := token.NewFileSet()
		file, err := parser.ParseFile(set, source.Path, source.Content, parser.ParseComments)
		if err != nil {
			return nil, fmt.Errorf("cannot parse build report source: %s", source.Path)
		}
		condition, err := sourceConstraint(file, source.Path)
		if err != nil {
			return nil, err
		}
		imports, err := sourceImports(file, packageNames)
		if err != nil {
			return nil, err
		}
		parsed[source.Path] = registryFile{file, set, source.Path, imports, condition}
	}
	report, exists := parsed["constant/features/tags.go"]
	if !exists {
		return []reportedBuildTag{}, nil
	}
	fail := func() ([]reportedBuildTag, error) {
		return nil, fmt.Errorf("build tag reporting requires source review")
	}
	if report.condition != "" {
		return fail()
	}
	var reporter *ast.FuncDecl
	for _, node := range report.file.Decls {
		if fn, ok := node.(*ast.FuncDecl); ok && fn.Name.Name == "Tags" {
			if reporter != nil || fn.Recv != nil || fn.Body == nil {
				return fail()
			}
			reporter = fn
		}
	}
	if reporter == nil {
		return fail()
	}
	signature, _ := expression(token.NewFileSet(), reporter.Type)
	if signature != "func() (tags []string)" {
		return fail()
	}

	caller, exists := parsed["main.go"]
	if !exists || caller.condition != "" || caller.imports["features"] != "github.com/metacubex/mihomo/constant/features" ||
		caller.imports["fmt"] != "fmt" || caller.imports["strings"] != "strings" {
		return fail()
	}
	const expectedCall = "if tags := features.Tags(); len(tags) != 0 {\n\tfmt.Printf(\"Use tags: %s\\n\", strings.Join(tags, \", \"))\n}"
	calls := []registryEvidence{}
	for _, node := range caller.file.Decls {
		fn, ok := node.(*ast.FuncDecl)
		if !ok || fn.Name.Name != "main" || fn.Recv != nil {
			continue
		}
		ast.Inspect(fn.Body, func(node ast.Node) bool {
			branch, ok := node.(*ast.IfStmt)
			if !ok {
				return true
			}
			shape, _ := expression(token.NewFileSet(), branch)
			if shape == expectedCall {
				evidence, _ := caller.evidence(branch, "main")
				calls = append(calls, evidence)
			}
			return true
		})
	}
	if len(calls) != 1 {
		return fail()
	}
	reportEvidence, err := report.evidence(reporter.Body, "Tags")
	if err != nil {
		return nil, err
	}
	tags := []reportedBuildTag{}
	seen := map[string]bool{}
	for index, node := range reporter.Body.List {
		if ret, ok := node.(*ast.ReturnStmt); ok && len(ret.Results) == 0 && index == len(reporter.Body.List)-1 {
			continue
		}
		branch, ok := node.(*ast.IfStmt)
		if !ok || branch.Init != nil || branch.Else != nil || len(branch.Body.List) != 1 {
			return fail()
		}
		symbol, ok := branch.Cond.(*ast.Ident)
		if !ok {
			return fail()
		}
		assignment, ok := branch.Body.List[0].(*ast.AssignStmt)
		if !ok || assignment.Tok != token.ASSIGN || len(assignment.Lhs) != 1 || len(assignment.Rhs) != 1 {
			return fail()
		}
		target, ok := assignment.Lhs[0].(*ast.Ident)
		if !ok || target.Name != "tags" {
			return fail()
		}
		call, ok := assignment.Rhs[0].(*ast.CallExpr)
		if !ok || len(call.Args) != 2 || call.Ellipsis.IsValid() {
			return fail()
		}
		name, ok := call.Fun.(*ast.Ident)
		if !ok || name.Name != "append" {
			return fail()
		}
		current, ok := call.Args[0].(*ast.Ident)
		if !ok || current.Name != "tags" {
			return fail()
		}
		tag, ok := stringLiteral(call.Args[1])
		if !ok || seen[tag] {
			return fail()
		}
		seen[tag] = true
		entry := reportedBuildTag{Tag: tag, Evidence: []registryEvidence{calls[0], reportEvidence}}
		enabled, disabled := 0, 0
		for _, file := range parsed {
			if !strings.HasPrefix(file.name, "constant/features/") {
				continue
			}
			for _, node := range file.file.Decls {
				group, ok := node.(*ast.GenDecl)
				if !ok || group.Tok != token.CONST {
					continue
				}
				for _, node := range group.Specs {
					spec, ok := node.(*ast.ValueSpec)
					if !ok || len(spec.Names) != 1 || spec.Names[0].Name != symbol.Name {
						continue
					}
					if len(spec.Values) != 1 {
						return fail()
					}
					value, ok := spec.Values[0].(*ast.Ident)
					if !ok || (value.Name != "true" && value.Name != "false") {
						return fail()
					}
					condition, err := constraint.Parse("//go:build " + file.condition)
					if err != nil {
						return fail()
					}
					if value.Name == "true" {
						positive, ok := condition.(*constraint.TagExpr)
						if !ok || positive.Tag != tag {
							return fail()
						}
						enabled++
					} else {
						negative, ok := condition.(*constraint.NotExpr)
						if !ok {
							return fail()
						}
						positive, ok := negative.X.(*constraint.TagExpr)
						if !ok || positive.Tag != tag {
							return fail()
						}
						disabled++
					}
					evidence, err := file.evidence(spec, symbol.Name)
					if err != nil {
						return nil, err
					}
					entry.Evidence = append(entry.Evidence, evidence)
				}
			}
		}
		if enabled != 1 || disabled != 1 {
			return fail()
		}
		sort.Slice(entry.Evidence, func(i, j int) bool { return entry.Evidence[i].Source.Path < entry.Evidence[j].Source.Path })
		tags = append(tags, entry)
	}
	if len(tags) == 0 || len(tags) > 128 {
		return fail()
	}
	sort.Slice(tags, func(i, j int) bool { return tags[i].Tag < tags[j].Tag })
	return tags, nil
}
