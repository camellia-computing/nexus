package main

import (
	"fmt"
	"go/ast"
	"go/token"
)

// ASCII comparison is the subset used by the registered protocol names.
func xrayDiscriminatorEvidence(file registryFile) (registryEvidence, error) {
	fail := func() (registryEvidence, error) {
		return registryEvidence{}, fmt.Errorf("Xray discriminator decoding requires review")
	}
	for _, item := range file.file.Decls {
		fn, ok := item.(*ast.FuncDecl)
		if !ok || fn.Name.Name != "LoadWithID" || fn.Recv == nil || len(fn.Recv.List) != 1 {
			continue
		}
		receiver := fn.Recv.List[0]
		pointer, ok := receiver.Type.(*ast.StarExpr)
		if !ok || len(receiver.Names) != 1 {
			continue
		}
		name, ok := pointer.X.(*ast.Ident)
		if !ok || name.Name != "JSONConfigLoader" {
			continue
		}
		if fn.Body == nil || len(fn.Body.List) < 2 {
			return fail()
		}
		assign, ok := fn.Body.List[0].(*ast.AssignStmt)
		if !ok || assign.Tok != token.ASSIGN || len(assign.Lhs) != 1 || len(assign.Rhs) != 1 {
			return fail()
		}
		id, ok := assign.Lhs[0].(*ast.Ident)
		if !ok {
			return fail()
		}
		call, ok := assign.Rhs[0].(*ast.CallExpr)
		if !ok || len(call.Args) != 1 {
			return fail()
		}
		selector, ok := call.Fun.(*ast.SelectorExpr)
		if !ok || selector.Sel.Name != "ToLower" {
			return fail()
		}
		imported, ok := selector.X.(*ast.Ident)
		if !ok || file.imports[imported.Name] != "strings" {
			return fail()
		}
		argument, ok := call.Args[0].(*ast.Ident)
		if !ok || argument.Name != id.Name {
			return fail()
		}
		create, ok := fn.Body.List[1].(*ast.AssignStmt)
		if !ok || len(create.Rhs) != 1 {
			return fail()
		}
		invoke, ok := create.Rhs[0].(*ast.CallExpr)
		if !ok || len(invoke.Args) != 1 {
			return fail()
		}
		argument, ok = invoke.Args[0].(*ast.Ident)
		if !ok || argument.Name != id.Name {
			return fail()
		}
		method, ok := invoke.Fun.(*ast.SelectorExpr)
		if !ok || method.Sel.Name != "CreateConfig" {
			return fail()
		}
		cache, ok := method.X.(*ast.SelectorExpr)
		if !ok || cache.Sel.Name != "cache" {
			return fail()
		}
		owner, ok := cache.X.(*ast.Ident)
		if !ok || owner.Name != receiver.Names[0].Name {
			return fail()
		}
		return file.evidence(fn.Body, "JSONConfigLoader.LoadWithID")
	}
	return fail()
}
