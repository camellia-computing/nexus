package main

import (
	"fmt"
	"go/ast"
	"go/token"
	"path"
	"sort"
	"strings"
)

type registrationFunction struct {
	file registryFile
	fn   *ast.FuncDecl
	info function
}

func combineConstraints(values ...string) string {
	unique := map[string]bool{}
	for _, value := range values {
		if value != "" {
			unique[value] = true
		}
	}
	terms := []string{}
	for term := range unique {
		terms = append(terms, "("+term+")")
	}
	sort.Strings(terms)
	return strings.Join(terms, " && ")
}

func alwaysRejects(body *ast.BlockStmt) bool {
	if body == nil || len(body.List) != 1 {
		return false
	}
	ret, ok := body.List[0].(*ast.ReturnStmt)
	if !ok || len(ret.Results) != 2 {
		return false
	}
	first, ok := ret.Results[0].(*ast.Ident)
	if !ok || first.Name != "nil" {
		return false
	}
	second, nilError := ret.Results[1].(*ast.Ident)
	return !(nilError && second.Name == "nil")
}

func singBoxBindings(files map[string]registryFile, functions []function) ([]decoderBinding, error) {
	const module = "github.com/sagernet/sing-box"
	index := map[string][]registrationFunction{}
	constants := map[string]string{}
	for _, file := range files {
		for _, item := range file.file.Decls {
			if fn, ok := item.(*ast.FuncDecl); ok && fn.Recv == nil {
				id := path.Dir(file.name) + "#" + fn.Name.Name
				for _, info := range functions {
					if info.ID == id && info.Source.Path == file.name {
						index[id] = append(index[id], registrationFunction{file, fn, info})
					}
				}
			}
			group, ok := item.(*ast.GenDecl)
			if !ok || group.Tok != token.CONST {
				continue
			}
			for _, item := range group.Specs {
				spec := item.(*ast.ValueSpec)
				for i, name := range spec.Names {
					if i < len(spec.Values) {
						if value, ok := stringLiteral(spec.Values[i]); ok {
							constants[path.Dir(file.name)+"#"+name.Name] = value
						}
					}
				}
			}
		}
	}
	bindings := []decoderBinding{}
	for id := range index {
		sort.Slice(index[id], func(i, j int) bool { return index[id][i].file.name < index[id][j].file.name })
	}
	for _, root := range []struct{ symbol, collection, adapter string }{
		{"InboundRegistry", "inbounds", "adapter/inbound"},
		{"OutboundRegistry", "outbounds", "adapter/outbound"},
	} {
		visits := 0
		var walk func(string, string, []registryEvidence, map[string]bool) error
		walk = func(id, condition string, evidence []registryEvidence, stack map[string]bool) error {
			visits++
			if visits > 512 || stack[id] || len(index[id]) == 0 {
				return fmt.Errorf("sing-box registration graph is incomplete or cyclic: %s", id)
			}
			stack[id] = true
			defer delete(stack, id)
			for _, entry := range index[id] {
				condition := combineConstraints(condition, entry.info.BuildConstraint)
				chain := append(append([]registryEvidence{}, evidence...), registryEvidence{entry.info.Source, entry.info.BodyHash})
				processed := map[*ast.CallExpr]bool{}
				for _, stmt := range entry.fn.Body.List {
					expr, ok := stmt.(*ast.ExprStmt)
					if !ok {
						continue
					}
					call, ok := expr.X.(*ast.CallExpr)
					if !ok || len(call.Args) == 0 {
						continue
					}
					registry, ok := call.Args[0].(*ast.Ident)
					if !ok || registry.Name != "registry" {
						continue
					}
					processed[call] = true
					generic, isRegister := call.Fun.(*ast.IndexExpr)
					if !isRegister {
						ref, err := entry.file.reference(call.Fun, module)
						if err != nil || len(call.Args) != 1 {
							return fmt.Errorf("sing-box registration call requires review: %s", id)
						}
						if err := walk(ref, condition, chain, stack); err != nil {
							return err
						}
						continue
					}
					ref, err := entry.file.reference(generic.X, module)
					if err != nil || ref != root.adapter+"#Register" || len(call.Args) != 3 {
						return fmt.Errorf("sing-box typed registration requires review: %s", id)
					}
					decl, err := entry.file.reference(generic.Index, module)
					if err != nil {
						return err
					}
					value, literal := stringLiteral(call.Args[1])
					if !literal {
						key, err := entry.file.reference(call.Args[1], module)
						if err != nil || constants[key] == "" {
							return fmt.Errorf("sing-box discriminator requires review: %s", id)
						}
						value = constants[key]
					}
					constructor, status := "", "registered"
					if fn, ok := call.Args[2].(*ast.FuncLit); ok {
						if !alwaysRejects(fn.Body) {
							return fmt.Errorf("sing-box inline constructor requires review: %s", id)
						}
						status = "rejected"
					} else {
						constructor, err = entry.file.reference(call.Args[2], module)
						if err != nil || len(index[constructor]) == 0 {
							return fmt.Errorf("sing-box constructor is missing: %s", id)
						}
					}
					bindings = append(bindings, decoderBinding{Path: []string{root.collection, "*"}, Discriminator: "type", DiscriminatorComparison: "exact", Value: value, OptionsPath: []string{}, Declaration: decl, Encoding: "json", BuildConstraint: condition, Constructor: constructor, ConstructorStatus: status, Evidence: chain})
				}
				unreviewed := false
				ast.Inspect(entry.fn.Body, func(node ast.Node) bool {
					if _, ok := node.(*ast.FuncLit); ok {
						return false
					}
					if call, ok := node.(*ast.CallExpr); ok && len(call.Args) > 0 {
						if arg, ok := call.Args[0].(*ast.Ident); ok && arg.Name == "registry" && !processed[call] {
							unreviewed = true
						}
					}
					return true
				})
				if unreviewed {
					return fmt.Errorf("sing-box conditional registration requires review: %s", id)
				}
			}
			return nil
		}
		if err := walk("include#"+root.symbol, "", nil, map[string]bool{}); err != nil {
			return nil, err
		}
	}
	return bindings, nil
}
