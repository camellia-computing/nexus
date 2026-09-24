package main

import (
	"fmt"
	"go/ast"
)

func mihomoObjectLayout(file, structure registryFile, fn *ast.FuncDecl) (*decodedObjectLayout, []registryEvidence, error) {
	fail := func() (*decodedObjectLayout, []registryEvidence, error) {
		return nil, nil, fmt.Errorf("Mihomo object decoding requires review")
	}
	layout := &decodedObjectLayout{EnvelopeDeclarations: []string{}, EnvelopeKeyComparison: "exact", SharedOptions: []decodedSharedOption{}, KeyComparison: "asciiCaseInsensitive"}
	replaceKeys, invalid := false, false
	ast.Inspect(fn.Body, func(node ast.Node) bool {
		pair, ok := node.(*ast.KeyValueExpr)
		if !ok {
			return true
		}
		key, ok := pair.Key.(*ast.Ident)
		if !ok || key.Name != "KeyReplacer" {
			return true
		}
		ref, err := file.reference(pair.Value, "github.com/metacubex/mihomo")
		if err != nil || ref != "common/structure#DefaultKeyReplacer" {
			invalid = true
		}
		replaceKeys = true
		return true
	})
	if invalid {
		return fail()
	}
	evidence := []registryEvidence{}
	decoderFound, entryFound, replacementFound := false, false, false
	for _, item := range structure.file.Decls {
		if method, ok := item.(*ast.FuncDecl); ok && (method.Name.Name == "decodeStructFromMap" || method.Name.Name == "Decode") {
			if method.Recv == nil || len(method.Recv.List) != 1 || receiverName(method.Recv.List[0].Type) != "Decoder" || method.Body == nil {
				return fail()
			}
			folded, embedded, tagged, replaced := false, false, false, false
			ast.Inspect(method.Body, func(node ast.Node) bool {
				if selector, ok := node.(*ast.SelectorExpr); ok && selector.Sel.Name == "Anonymous" {
					embedded = true
				}
				call, ok := node.(*ast.CallExpr)
				if !ok {
					return true
				}
				selector, ok := call.Fun.(*ast.SelectorExpr)
				if !ok {
					return true
				}
				if owner, ok := selector.X.(*ast.Ident); ok && structure.imports[owner.Name] == "strings" && selector.Sel.Name == "EqualFold" && len(call.Args) == 2 {
					folded = true
				}
				if selector.Sel.Name == "Get" && len(call.Args) == 1 {
					if tag, ok := call.Args[0].(*ast.SelectorExpr); ok && tag.Sel.Name == "TagName" {
						tagged = true
					}
				}
				if selector.Sel.Name == "Replace" {
					if owner, ok := selector.X.(*ast.SelectorExpr); ok && owner.Sel.Name == "KeyReplacer" {
						replaced = true
					}
				}
				return true
			})
			if method.Name.Name == "decodeStructFromMap" {
				if !folded || !tagged || (replaceKeys && !replaced) {
					return fail()
				}
				layout.NestedEmbedded = embedded
				decoderFound = true
			} else {
				delegate := false
				for _, stmt := range method.Body.List {
					ret, ok := stmt.(*ast.ReturnStmt)
					if !ok || len(ret.Results) != 1 {
						continue
					}
					call, ok := ret.Results[0].(*ast.CallExpr)
					if !ok || len(call.Args) != 3 {
						continue
					}
					if selector, ok := call.Fun.(*ast.SelectorExpr); ok && selector.Sel.Name == "decode" {
						delegate = true
					}
				}
				if !delegate && (!embedded || !folded || !tagged || (replaceKeys && !replaced)) {
					return fail()
				}
				layout.RootFieldNames = delegate
				entryFound = true
			}
			ref, err := structure.evidence(method.Body, "Decoder."+method.Name.Name)
			if err != nil {
				return nil, nil, err
			}
			evidence = append(evidence, ref)
		}
		group, ok := item.(*ast.GenDecl)
		if !ok {
			continue
		}
		for _, entry := range group.Specs {
			value, ok := entry.(*ast.ValueSpec)
			if !ok || len(value.Names) != 1 || value.Names[0].Name != "DefaultKeyReplacer" || len(value.Values) != 1 {
				continue
			}
			call, ok := value.Values[0].(*ast.CallExpr)
			if !ok || len(call.Args) != 2 {
				return fail()
			}
			method, ok := call.Fun.(*ast.SelectorExpr)
			if !ok || method.Sel.Name != "NewReplacer" {
				return fail()
			}
			owner, ok := method.X.(*ast.Ident)
			if !ok || structure.imports[owner.Name] != "strings" {
				return fail()
			}
			from, _ := stringLiteral(call.Args[0])
			to, _ := stringLiteral(call.Args[1])
			if from != "_" || to != "-" {
				return fail()
			}
			if replaceKeys {
				ref, err := structure.evidence(call, "DefaultKeyReplacer")
				if err != nil {
					return nil, nil, err
				}
				evidence = append(evidence, ref)
			}
			replacementFound = true
		}
	}
	if !decoderFound || !entryFound || (layout.RootFieldNames && !layout.NestedEmbedded) || (replaceKeys && !replacementFound) {
		return fail()
	}
	if replaceKeys {
		layout.KeyComparison = "asciiCaseInsensitiveUnderscore"
	}
	for _, statement := range fn.Body.List {
		condition, ok := statement.(*ast.IfStmt)
		if !ok || condition.Init == nil {
			continue
		}
		assign, ok := condition.Init.(*ast.AssignStmt)
		if !ok || len(assign.Rhs) != 1 || len(assign.Lhs) != 2 {
			continue
		}
		assertion, ok := assign.Rhs[0].(*ast.TypeAssertExpr)
		if !ok {
			continue
		}
		index, ok := assertion.X.(*ast.IndexExpr)
		if !ok {
			continue
		}
		mapping, ok := index.X.(*ast.Ident)
		if !ok || mapping.Name != "mapping" {
			continue
		}
		key, ok := stringLiteral(index.Index)
		if !ok {
			return fail()
		}
		mapType, ok := assertion.Type.(*ast.MapType)
		if !ok {
			return fail()
		}
		mapKey, ok := mapType.Key.(*ast.Ident)
		if !ok || mapKey.Name != "string" {
			return fail()
		}
		input, ok := assign.Lhs[0].(*ast.Ident)
		if !ok {
			return fail()
		}
		variables := map[string]ast.Expr{}
		decoded := ""
		for _, statement := range condition.Body.List {
			assign, ok := statement.(*ast.AssignStmt)
			if !ok || len(assign.Rhs) != 1 {
				continue
			}
			if len(assign.Lhs) == 1 && constructedType(assign.Rhs[0]) != nil {
				if name, ok := assign.Lhs[0].(*ast.Ident); ok {
					variables[name.Name] = constructedType(assign.Rhs[0])
				}
			}
			call, ok := assign.Rhs[0].(*ast.CallExpr)
			if !ok {
				continue
			}
			method, ok := call.Fun.(*ast.SelectorExpr)
			if !ok || method.Sel.Name != "Decode" || len(call.Args) != 2 {
				continue
			}
			decoder, ok := method.X.(*ast.Ident)
			if !ok || decoder.Name != "decoder" {
				return fail()
			}
			value, validValue := call.Args[0].(*ast.Ident)
			option, validOption := call.Args[1].(*ast.Ident)
			if !validValue || value.Name != input.Name || !validOption || variables[option.Name] == nil || decoded != "" {
				return fail()
			}
			var err error
			decoded, err = file.reference(variables[option.Name], "github.com/metacubex/mihomo")
			if err != nil {
				return nil, nil, err
			}
		}
		if decoded == "" {
			return fail()
		}
		layout.SharedOptions = append(layout.SharedOptions, decodedSharedOption{Path: []string{key}, Declaration: decoded})
	}
	knownKeys := map[string]bool{"type": true}
	for _, shared := range layout.SharedOptions {
		if knownKeys[shared.Path[0]] {
			return fail()
		}
		knownKeys[shared.Path[0]] = true
	}
	ast.Inspect(fn.Body, func(node ast.Node) bool {
		if index, ok := node.(*ast.IndexExpr); ok {
			if mapping, ok := index.X.(*ast.Ident); ok && mapping.Name == "mapping" {
				key, ok := stringLiteral(index.Index)
				if !ok || !knownKeys[key] {
					invalid = true
				}
			}
		}
		return true
	})
	if invalid {
		return fail()
	}
	return layout, evidence, nil
}
