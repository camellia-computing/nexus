package main

import (
	"crypto/sha256"
	"fmt"
	"go/ast"
	"go/parser"
	"go/token"
	"path"
	"sort"
	"strconv"
	"strings"
)

// A decoder binding proves the source dispatches a discriminator to a declaration.
// It does not prove the installed binary includes the constructor or accepts every field.
type decoderBinding struct {
	Path                    []string             `json:"path"`
	Discriminator           string               `json:"discriminator"`
	DiscriminatorComparison string               `json:"discriminatorComparison"`
	Value                   string               `json:"value"`
	OptionsPath             []string             `json:"optionsPath"`
	Declaration             string               `json:"declaration"`
	Encoding                string               `json:"encoding"`
	ObjectLayout            *decodedObjectLayout `json:"objectLayout"`
	BuildConstraint         string               `json:"buildConstraint"`
	Constructor             string               `json:"constructor"`
	ConstructorStatus       string               `json:"constructorStatus"`
	Evidence                []registryEvidence   `json:"evidence"`
}

type decodedObjectLayout struct {
	BuildConstraint       string                `json:"buildConstraint"`
	EnvelopeDeclarations  []string              `json:"envelopeDeclarations"`
	EnvelopeKeyComparison string                `json:"envelopeKeyComparison"`
	SharedOptions         []decodedSharedOption `json:"sharedOptions"`
	KeyComparison         string                `json:"keyComparison"`
	RootFieldNames        bool                  `json:"rootFieldNames"`
	NestedEmbedded        bool                  `json:"nestedEmbedded"`
}

type decodedSharedOption struct {
	Path        []string `json:"path"`
	Declaration string   `json:"declaration"`
}

type registryEvidence struct {
	Source   sourceRef `json:"source"`
	BodyHash string    `json:"bodyHash"`
}

type registryFile struct {
	file      *ast.File
	set       *token.FileSet
	name      string
	imports   map[string]string
	condition string
}

func (f registryFile) reference(expr ast.Expr, module string) (string, error) {
	switch item := expr.(type) {
	case *ast.Ident:
		return path.Dir(f.name) + "#" + item.Name, nil
	case *ast.SelectorExpr:
		if base, ok := item.X.(*ast.Ident); ok {
			if imported := f.imports[base.Name]; strings.HasPrefix(imported, module+"/") {
				return strings.TrimPrefix(imported, module+"/") + "#" + item.Sel.Name, nil
			}
		}
	}
	return "", fmt.Errorf("unresolved registry reference in %s", f.name)
}

func (f registryFile) evidence(node ast.Node, symbol string) (registryEvidence, error) {
	body, err := expression(token.NewFileSet(), node)
	return registryEvidence{sourceRef{f.name, f.set.Position(node.Pos()).Line, symbol}, fmt.Sprintf("%x", sha256.Sum256([]byte(body)))}, err
}

func stringLiteral(expr ast.Expr) (string, bool) {
	literal, ok := expr.(*ast.BasicLit)
	if !ok || literal.Kind != token.STRING {
		return "", false
	}
	value, err := strconv.Unquote(literal.Value)
	return value, err == nil && value != ""
}

func constructedType(expr ast.Expr) ast.Expr {
	switch item := expr.(type) {
	case *ast.UnaryExpr:
		if item.Op == token.AND {
			return constructedType(item.X)
		}
	case *ast.CompositeLit:
		return item.Type
	case *ast.CallExpr:
		if name, ok := item.Fun.(*ast.Ident); ok && name.Name == "new" && len(item.Args) == 1 {
			return item.Args[0]
		}
	}
	return nil
}

func extractDecoderBindings(files []sourceFile, functions []function, packageNames map[string]string) ([]decoderBinding, error) {
	parsed := map[string]registryFile{}
	for _, source := range files {
		set := token.NewFileSet()
		file, err := parser.ParseFile(set, source.Path, source.Content, parser.ParseComments)
		if err != nil {
			return nil, fmt.Errorf("cannot parse registry source: %s", source.Path)
		}
		imports, err := sourceImports(file, packageNames)
		if err != nil {
			return nil, err
		}
		condition, err := sourceConstraint(file, source.Path)
		if err != nil {
			return nil, err
		}
		parsed[source.Path] = registryFile{file, set, source.Path, imports, condition}
	}
	bindings := []decoderBinding{}
	if file, ok := parsed["infra/conf/xray.go"]; ok {
		loader, found := parsed["infra/conf/loader.go"]
		if !found {
			return nil, fmt.Errorf("Xray decoder implementation is missing")
		}
		entries, err := xrayBindings(file, loader)
		if err != nil {
			return nil, err
		}
		bindings = append(bindings, entries...)
	}
	if file, ok := parsed["adapter/parser.go"]; ok {
		structure, found := parsed["common/structure/structure.go"]
		if !found {
			return nil, fmt.Errorf("Mihomo structure decoder is missing")
		}
		entries, err := mihomoBindings(file, structure)
		if err != nil {
			return nil, err
		}
		bindings = append(bindings, entries...)
	}
	if _, ok := parsed["include/registry.go"]; ok {
		entries, err := singBoxBindings(parsed, functions)
		if err != nil {
			return nil, err
		}
		bindings = append(bindings, entries...)
	}
	resolved := []decoderBinding{}
	for _, binding := range bindings {
		if len(binding.OptionsPath) > 0 {
			binding.ObjectLayout = &decodedObjectLayout{EnvelopeDeclarations: []string{}, EnvelopeKeyComparison: "exact", SharedOptions: []decodedSharedOption{}, KeyComparison: "asciiCaseInsensitive", RootFieldNames: true, NestedEmbedded: true}
		}
		if binding.ConstructorStatus == "rejected" {
			resolved = append(resolved, binding)
			continue
		}
		found := false
		for _, fn := range functions {
			if fn.ID == binding.Constructor {
				found = true
				variant := binding
				variant.BuildConstraint = combineConstraints(binding.BuildConstraint, fn.BuildConstraint)
				for _, node := range parsed[fn.Source.Path].file.Decls {
					declaration, ok := node.(*ast.FuncDecl)
					if ok && parsed[fn.Source.Path].set.Position(declaration.Pos()).Line == fn.Source.Line && alwaysRejects(declaration.Body) {
						variant.ConstructorStatus = "rejected"
					}
				}
				variant.Evidence = append(append([]registryEvidence{}, binding.Evidence...), registryEvidence{fn.Source, fn.BodyHash})
				resolved = append(resolved, variant)
			}
		}
		if !found {
			return nil, fmt.Errorf("decoder constructor source is missing: %s", binding.Constructor)
		}
	}
	bindings = resolved
	sort.Slice(bindings, func(i, j int) bool {
		a, b := bindings[i], bindings[j]
		return strings.Join(a.Path, "/")+a.Value+a.BuildConstraint+a.Declaration < strings.Join(b.Path, "/")+b.Value+b.BuildConstraint+b.Declaration
	})
	return bindings, nil
}

func xrayBindings(file, loader registryFile) ([]decoderBinding, error) {
	decodeEvidence, err := xrayDiscriminatorEvidence(loader)
	if err != nil {
		return nil, err
	}
	bindings := []decoderBinding{}
	for _, item := range file.file.Decls {
		group, ok := item.(*ast.GenDecl)
		if !ok || group.Tok != token.VAR {
			continue
		}
		for _, item := range group.Specs {
			spec := item.(*ast.ValueSpec)
			if len(spec.Names) != 1 || (spec.Names[0].Name != "inboundConfigLoader" && spec.Names[0].Name != "outboundConfigLoader") {
				continue
			}
			fail := func() ([]decoderBinding, error) { return nil, fmt.Errorf("Xray decoder registry requires review") }
			if len(spec.Values) != 1 {
				return fail()
			}
			call, ok := spec.Values[0].(*ast.CallExpr)
			if !ok || len(call.Args) != 3 {
				return fail()
			}
			name, ok := call.Fun.(*ast.Ident)
			if !ok || name.Name != "NewJSONConfigLoader" {
				return fail()
			}
			discriminator, validDiscriminator := stringLiteral(call.Args[1])
			optionsPath, validOptions := stringLiteral(call.Args[2])
			cache, ok := call.Args[0].(*ast.CompositeLit)
			if !ok || !validDiscriminator || !validOptions {
				return fail()
			}
			cacheType, ok := cache.Type.(*ast.Ident)
			if !ok || cacheType.Name != "ConfigCreatorCache" {
				return fail()
			}
			evidence, err := file.evidence(call, spec.Names[0].Name)
			if err != nil {
				return nil, err
			}
			for _, item := range cache.Elts {
				entry, ok := item.(*ast.KeyValueExpr)
				if !ok {
					return fail()
				}
				value, ok := stringLiteral(entry.Key)
				factory, isFactory := entry.Value.(*ast.FuncLit)
				if !ok || !isFactory || len(factory.Body.List) != 1 {
					return fail()
				}
				ret, ok := factory.Body.List[0].(*ast.ReturnStmt)
				if !ok || len(ret.Results) != 1 || constructedType(ret.Results[0]) == nil {
					return fail()
				}
				decl, err := file.reference(constructedType(ret.Results[0]), "github.com/xtls/xray-core")
				if err != nil {
					return nil, err
				}
				collection := strings.TrimSuffix(spec.Names[0].Name, "ConfigLoader") + "s"
				bindings = append(bindings, decoderBinding{Path: []string{collection, "*"}, Discriminator: discriminator, DiscriminatorComparison: "asciiCaseInsensitive", Value: value, OptionsPath: []string{optionsPath}, Declaration: decl, Encoding: "json", BuildConstraint: file.condition, Constructor: decl + ".Build", ConstructorStatus: "registered", Evidence: []registryEvidence{evidence, decodeEvidence}})
			}
		}
	}
	if len(bindings) == 0 {
		return nil, fmt.Errorf("Xray decoder registries are missing")
	}
	return bindings, nil
}

func mihomoBindings(file, structure registryFile) ([]decoderBinding, error) {
	bindings := []decoderBinding{}
	for _, item := range file.file.Decls {
		fn, ok := item.(*ast.FuncDecl)
		if !ok || fn.Name.Name != "ParseProxy" || fn.Body == nil {
			continue
		}
		evidence, err := file.evidence(fn.Body, fn.Name.Name)
		if err != nil {
			return nil, err
		}
		decoderIsProxy, discriminatorMatches := false, false
		ast.Inspect(fn.Body, func(node ast.Node) bool {
			assignment, ok := node.(*ast.AssignStmt)
			if !ok || len(assignment.Lhs) == 0 || len(assignment.Rhs) != 1 {
				return true
			}
			name, ok := assignment.Lhs[0].(*ast.Ident)
			if !ok {
				return true
			}
			if name.Name == "proxyType" {
				if assertion, ok := assignment.Rhs[0].(*ast.TypeAssertExpr); ok {
					if index, ok := assertion.X.(*ast.IndexExpr); ok {
						mapping, validMapping := index.X.(*ast.Ident)
						typeName, validType := assertion.Type.(*ast.Ident)
						key, _ := stringLiteral(index.Index)
						discriminatorMatches = validMapping && mapping.Name == "mapping" && validType && typeName.Name == "string" && key == "type"
					}
				}
			}
			call, ok := assignment.Rhs[0].(*ast.CallExpr)
			if !ok || name.Name != "decoder" {
				return true
			}
			selector, ok := call.Fun.(*ast.SelectorExpr)
			if !ok || selector.Sel.Name != "NewDecoder" || len(call.Args) != 1 {
				return true
			}
			ref, err := file.reference(selector, "github.com/metacubex/mihomo")
			if err != nil || ref != "common/structure#NewDecoder" {
				return true
			}
			options, ok := call.Args[0].(*ast.CompositeLit)
			if ok {
				for _, entry := range options.Elts {
					if pair, ok := entry.(*ast.KeyValueExpr); ok {
						if key, ok := pair.Key.(*ast.Ident); ok && key.Name == "TagName" {
							value, _ := stringLiteral(pair.Value)
							decoderIsProxy = value == "proxy"
						}
					}
				}
			}
			return true
		})
		if !decoderIsProxy || !discriminatorMatches {
			return nil, fmt.Errorf("Mihomo proxy tag decoder requires review")
		}
		layout, layoutEvidence, err := mihomoObjectLayout(file, structure, fn)
		if err != nil {
			return nil, err
		}
		for _, stmt := range fn.Body.List {
			switchStmt, ok := stmt.(*ast.SwitchStmt)
			if !ok {
				continue
			}
			name, ok := switchStmt.Tag.(*ast.Ident)
			if !ok || name.Name != "proxyType" {
				continue
			}
			for _, item := range switchStmt.Body.List {
				clause := item.(*ast.CaseClause)
				if clause.List == nil {
					continue
				}
				variables := map[string]ast.Expr{}
				decoded, decodedVariable, constructor := "", "", ""
				for _, stmt := range clause.Body {
					assignment, ok := stmt.(*ast.AssignStmt)
					if !ok {
						continue
					}
					if len(assignment.Lhs) == 1 && len(assignment.Rhs) == 1 {
						if name, ok := assignment.Lhs[0].(*ast.Ident); ok && constructedType(assignment.Rhs[0]) != nil {
							variables[name.Name] = constructedType(assignment.Rhs[0])
						}
					}
					for _, rhs := range assignment.Rhs {
						call, ok := rhs.(*ast.CallExpr)
						if !ok {
							continue
						}
						selector, ok := call.Fun.(*ast.SelectorExpr)
						if !ok {
							continue
						}
						if base, ok := selector.X.(*ast.Ident); ok && base.Name == "decoder" && selector.Sel.Name == "Decode" && len(call.Args) == 2 {
							mapping, isMapping := call.Args[0].(*ast.Ident)
							option, isOption := call.Args[1].(*ast.Ident)
							if !isMapping || mapping.Name != "mapping" || !isOption || variables[option.Name] == nil || decoded != "" {
								return nil, fmt.Errorf("Mihomo proxy decode binding requires review")
							}
							decoded, err = file.reference(variables[option.Name], "github.com/metacubex/mihomo")
							decodedVariable = option.Name
							if err != nil {
								return nil, err
							}
						} else if ref, err := file.reference(selector, "github.com/metacubex/mihomo"); err == nil && strings.HasPrefix(ref, "adapter/outbound#New") {
							usesDecoded := false
							if len(call.Args) > 0 {
								if dereference, ok := call.Args[0].(*ast.StarExpr); ok {
									if name, ok := dereference.X.(*ast.Ident); ok {
										usesDecoded = name.Name == decodedVariable
									}
								}
							}
							if constructor != "" || !usesDecoded {
								return nil, fmt.Errorf("Mihomo proxy constructor is ambiguous")
							}
							constructor = ref
						}
					}
				}
				if decoded == "" || constructor == "" {
					return nil, fmt.Errorf("Mihomo proxy constructor binding requires review")
				}
				for _, expr := range clause.List {
					value, ok := stringLiteral(expr)
					if !ok {
						return nil, fmt.Errorf("Mihomo proxy discriminator requires review")
					}
					bindings = append(bindings, decoderBinding{Path: []string{"proxies", "*"}, Discriminator: "type", DiscriminatorComparison: "exact", Value: value, OptionsPath: []string{}, Declaration: decoded, Encoding: "proxy", ObjectLayout: layout, BuildConstraint: file.condition, Constructor: constructor, ConstructorStatus: "registered", Evidence: append([]registryEvidence{evidence}, layoutEvidence...)})
				}
			}
		}
	}
	if len(bindings) == 0 {
		return nil, fmt.Errorf("Mihomo proxy registry is missing")
	}
	return bindings, nil
}
