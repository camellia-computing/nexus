// Command core-knowledge-extractor inventories configuration declarations without executing them.
package main

import (
	"bytes"
	"crypto/sha256"
	"encoding/json"
	"fmt"
	"go/ast"
	"go/build/constraint"
	"go/format"
	"go/parser"
	"go/token"
	"io"
	"os"
	"path"
	"reflect"
	"sort"
	"strconv"
	"strings"
)

const maxInputBytes = 64 << 20

type sourceFile struct {
	Path    string `json:"path"`
	Content string `json:"content"`
}

type sourceRequest struct {
	ModulePath string       `json:"modulePath"`
	Files      []sourceFile `json:"files"`
}

func sourcePackageNames(files []sourceFile, modulePath string) (map[string]string, error) {
	if modulePath == "" || modulePath == "." || modulePath == ".." || path.Clean(modulePath) != modulePath || path.IsAbs(modulePath) || strings.ContainsAny(modulePath, "\\ \t\r\n") || strings.HasPrefix(modulePath, "../") {
		return nil, fmt.Errorf("invalid source module identity")
	}
	names := map[string]string{}
	for _, source := range files {
		if path.Clean(source.Path) != source.Path || strings.HasPrefix(source.Path, "../") || path.IsAbs(source.Path) || strings.Contains(source.Path, "\\") {
			return nil, fmt.Errorf("invalid source path")
		}
		file, err := parser.ParseFile(token.NewFileSet(), source.Path, source.Content, parser.PackageClauseOnly)
		if err != nil {
			return nil, fmt.Errorf("cannot read source package: %s", source.Path)
		}
		// Executable packages, including source generators, cannot supply an imported package.
		if file.Name.Name == "main" {
			continue
		}
		identity := path.Join(modulePath, path.Dir(source.Path))
		if previous, found := names[identity]; found && previous != file.Name.Name {
			return nil, fmt.Errorf("source package names require review: %s", source.Path)
		}
		names[identity] = file.Name.Name
	}
	return names, nil
}

func sourceImports(file *ast.File, packageNames map[string]string) (map[string]string, error) {
	imports := map[string]string{}
	for _, item := range file.Imports {
		value, err := strconv.Unquote(item.Path.Value)
		if err != nil {
			return nil, fmt.Errorf("invalid Go import")
		}
		name := path.Base(value)
		if declared, found := packageNames[value]; found {
			name = declared
		}
		if item.Name != nil {
			name = item.Name.Name
		}
		if name == "_" || name == "." {
			continue
		}
		if _, found := imports[name]; found {
			return nil, fmt.Errorf("ambiguous Go import binding")
		}
		imports[name] = value
	}
	return imports, nil
}

type sourceRef struct {
	Path   string `json:"path"`
	Line   int    `json:"line"`
	Symbol string `json:"symbol"`
}

type field struct {
	Name        string            `json:"name"`
	Type        string            `json:"type"`
	Shape       typeShape         `json:"shape"`
	Embedded    bool              `json:"embedded"`
	Tags        map[string]string `json:"tags"`
	Annotations map[string]string `json:"annotations"`
	Source      sourceRef         `json:"source"`
}

type declaration struct {
	ID              string            `json:"id"`
	Name            string            `json:"name"`
	Package         string            `json:"package"`
	Type            string            `json:"type"`
	Shape           typeShape         `json:"shape"`
	Alias           bool              `json:"alias"`
	BuildConstraint string            `json:"buildConstraint"`
	Imports         map[string]string `json:"imports"`
	Fields          []field           `json:"fields"`
	Methods         []string          `json:"methods"`
	Source          sourceRef         `json:"source"`
}

type method struct {
	Receiver string    `json:"receiver"`
	Name     string    `json:"name"`
	Source   sourceRef `json:"source"`
}

type inventory struct {
	Declarations      []declaration      `json:"declarations"`
	Methods           []method           `json:"methods"`
	Functions         []function         `json:"functions"`
	DecoderBindings   []decoderBinding   `json:"decoderBindings"`
	ReportedBuildTags []reportedBuildTag `json:"reportedBuildTags"`
}

type function struct {
	ID              string            `json:"id"`
	BodyHash        string            `json:"bodyHash"`
	BuildConstraint string            `json:"buildConstraint"`
	Source          sourceRef         `json:"source"`
	Imports         map[string]string `json:"imports"`
}

// Go filename constraints participate even when the file has no go:build line.
func fileConstraint(name, explicit string) string {
	osTags := strings.Fields("aix android darwin dragonfly freebsd hurd illumos ios js linux nacl netbsd openbsd plan9 solaris wasip1 windows zos")
	archTags := strings.Fields("386 amd64 amd64p32 arm armbe arm64 arm64be loong64 mips mipsle mips64 mips64le mips64p32 mips64p32le ppc ppc64 ppc64le riscv riscv64 s390 s390x sparc sparc64 wasm")
	known := func(value string, tags []string) bool {
		for _, tag := range tags {
			if tag == value {
				return true
			}
		}
		return false
	}
	parts := strings.Split(strings.TrimSuffix(path.Base(name), ".go"), "_")
	conditions := []string{}
	if len(parts) > 1 {
		last := parts[len(parts)-1]
		if known(last, archTags) {
			if len(parts) > 2 && known(parts[len(parts)-2], osTags) {
				conditions = append(conditions, parts[len(parts)-2])
			}
			conditions = append(conditions, last)
		} else if known(last, osTags) {
			conditions = append(conditions, last)
		}
	}
	if explicit != "" {
		conditions = append(conditions, "("+explicit+")")
	}
	return strings.Join(conditions, " && ")
}

func sourceConstraint(file *ast.File, name string) (string, error) {
	explicit, additional := "", ""
	for _, group := range file.Comments {
		if group.Pos() >= file.Package {
			break
		}
		for _, comment := range group.List {
			if !constraint.IsGoBuild(comment.Text) && !constraint.IsPlusBuild(comment.Text) {
				continue
			}
			parsed, err := constraint.Parse(comment.Text)
			if err != nil {
				return "", fmt.Errorf("invalid Go build constraint: %s", name)
			}
			if constraint.IsGoBuild(comment.Text) {
				if explicit != "" {
					return "", fmt.Errorf("duplicate Go build constraint: %s", name)
				}
				explicit = parsed.String()
			} else {
				additional = combineConstraints(additional, parsed.String())
			}
		}
	}
	if explicit == "" {
		explicit = additional
	}
	for _, item := range file.Imports {
		value, _ := strconv.Unquote(item.Path.Value)
		if value == "C" {
			explicit = combineConstraints(explicit, "cgo")
		}
	}
	return fileConstraint(name, explicit), nil
}

func expression(set *token.FileSet, node ast.Node) (string, error) {
	var buffer bytes.Buffer
	if err := format.Node(&buffer, set, node); err != nil {
		return "", err
	}
	return buffer.String(), nil
}

func receiverName(expr ast.Expr) string {
	switch item := expr.(type) {
	case *ast.Ident:
		return item.Name
	case *ast.StarExpr:
		return receiverName(item.X)
	case *ast.IndexExpr:
		return receiverName(item.X)
	case *ast.IndexListExpr:
		return receiverName(item.X)
	default:
		return ""
	}
}

func extractModule(files []sourceFile, modulePath string) (inventory, error) {
	result := inventory{Declarations: []declaration{}, Methods: []method{}, Functions: []function{}}
	packageNames, err := sourcePackageNames(files, modulePath)
	if err != nil {
		return result, err
	}
	seen := map[string]bool{}
	for _, source := range files {
		if path.Clean(source.Path) != source.Path || strings.HasPrefix(source.Path, "../") || path.IsAbs(source.Path) || strings.Contains(source.Path, "\\") || seen[source.Path] {
			return result, fmt.Errorf("invalid or duplicate source path")
		}
		seen[source.Path] = true
		set := token.NewFileSet()
		file, err := parser.ParseFile(set, source.Path, source.Content, parser.ParseComments|parser.AllErrors)
		if err != nil {
			// Do not echo source text from a parse failure.
			return result, fmt.Errorf("cannot parse Go source: %s", source.Path)
		}
		packagePath := path.Dir(source.Path)
		imports, err := sourceImports(file, packageNames)
		if err != nil {
			return result, err
		}
		buildConstraint, err := sourceConstraint(file, source.Path)
		if err != nil {
			return result, err
		}
		initOrdinal := 0
		for _, item := range file.Decls {
			if fn, ok := item.(*ast.FuncDecl); ok {
				symbol := fn.Name.Name
				if symbol == "init" && fn.Recv == nil {
					initOrdinal++
					symbol = fmt.Sprintf("init@%d", initOrdinal)
				}
				if fn.Recv != nil && len(fn.Recv.List) == 1 {
					receiver := receiverName(fn.Recv.List[0].Type)
					if receiver != "" {
						symbol = receiver + "." + fn.Name.Name
						result.Methods = append(result.Methods, method{packagePath + "#" + receiver, fn.Name.Name, sourceRef{source.Path, set.Position(fn.Pos()).Line, symbol}})
					}
				}
				if fn.Body != nil {
					// An empty FileSet removes layout differences; comments are not part of the body AST.
					body, err := expression(token.NewFileSet(), fn.Body)
					if err != nil {
						return result, err
					}
					result.Functions = append(result.Functions, function{packagePath + "#" + symbol, fmt.Sprintf("%x", sha256.Sum256([]byte(body))), buildConstraint, sourceRef{source.Path, set.Position(fn.Pos()).Line, symbol}, imports})
				}
				continue
			}
			group, ok := item.(*ast.GenDecl)
			if ok && (group.Tok == token.VAR || group.Tok == token.CONST) {
				for _, spec := range group.Specs {
					value := spec.(*ast.ValueSpec)
					if len(value.Names) != len(value.Values) {
						continue
					}
					for i, name := range value.Names {
						_, selector := value.Values[i].(*ast.SelectorExpr)
						identifier, literal := value.Values[i].(*ast.Ident)
						boolean := literal && (identifier.Name == "true" || identifier.Name == "false")
						if !selector && !boolean {
							continue
						}
						body, err := expression(token.NewFileSet(), value.Values[i])
						if err != nil {
							return result, err
						}
						result.Functions = append(result.Functions, function{packagePath + "#" + name.Name, fmt.Sprintf("%x", sha256.Sum256([]byte(body))), buildConstraint, sourceRef{source.Path, set.Position(name.Pos()).Line, name.Name}, imports})
					}
				}
			}
			if !ok || group.Tok != token.TYPE {
				continue
			}
			for _, spec := range group.Specs {
				typeSpec := spec.(*ast.TypeSpec)
				typeName, err := expression(set, typeSpec.Type)
				if err != nil {
					return result, err
				}
				entry := declaration{ID: packagePath + "#" + typeSpec.Name.Name, Name: typeSpec.Name.Name, Package: packagePath, Type: typeName, Shape: describeType(typeSpec.Type, packagePath, imports), Alias: typeSpec.Assign.IsValid(), BuildConstraint: buildConstraint, Imports: imports, Fields: []field{}, Methods: []string{}, Source: sourceRef{source.Path, set.Position(typeSpec.Pos()).Line, typeSpec.Name.Name}}
				entry.Imports = map[string]string{}
				ast.Inspect(typeSpec.Type, func(node ast.Node) bool {
					if selector, ok := node.(*ast.SelectorExpr); ok {
						if name, ok := selector.X.(*ast.Ident); ok {
							if imported, ok := imports[name.Name]; ok {
								entry.Imports[name.Name] = imported
							}
						}
					}
					return true
				})
				if structure, ok := typeSpec.Type.(*ast.StructType); ok {
					entry.Type = "struct"
					for _, item := range structure.Fields.List {
						fieldType, err := expression(set, item.Type)
						if err != nil {
							return result, err
						}
						tags := map[string]string{}
						annotations := map[string]string{}
						if item.Tag != nil {
							value, err := strconv.Unquote(item.Tag.Value)
							if err != nil {
								return result, fmt.Errorf("invalid Go struct tag: %s", source.Path)
							}
							for _, encoding := range []string{"json", "yaml", "proxy", "mapstructure"} {
								if value, ok := reflect.StructTag(value).Lookup(encoding); ok {
									tags[encoding] = value
								}
							}
							for _, annotation := range []string{"enum", "minimum", "maximum", "default", "required", "reference", "format", "schema"} {
								if value, ok := reflect.StructTag(value).Lookup(annotation); ok {
									annotations[annotation] = value
								}
							}
						}
						names := []string{}
						for _, name := range item.Names {
							names = append(names, name.Name)
						}
						if len(names) == 0 {
							names = append(names, fieldType)
						}
						for _, name := range names {
							entry.Fields = append(entry.Fields, field{Name: name, Type: fieldType, Shape: describeType(item.Type, packagePath, imports), Embedded: len(item.Names) == 0, Tags: tags, Annotations: annotations, Source: sourceRef{source.Path, set.Position(item.Pos()).Line, entry.Name + "." + name}})
						}
					}
				}
				result.Declarations = append(result.Declarations, entry)
			}
		}
	}
	for index := range result.Declarations {
		entry := &result.Declarations[index]
		for _, method := range result.Methods {
			if method.Receiver == entry.ID {
				entry.Methods = append(entry.Methods, method.Name)
			}
		}
		sort.Strings(entry.Methods)
	}
	sort.Slice(result.Declarations, func(i, j int) bool {
		a, b := result.Declarations[i], result.Declarations[j]
		if a.ID != b.ID {
			return a.ID < b.ID
		}
		return a.Source.Path < b.Source.Path
	})
	sort.Slice(result.Methods, func(i, j int) bool {
		a, b := result.Methods[i], result.Methods[j]
		return a.Receiver+"/"+a.Name+"/"+a.Source.Path < b.Receiver+"/"+b.Name+"/"+b.Source.Path
	})
	sort.Slice(result.Functions, func(i, j int) bool {
		a, b := result.Functions[i], result.Functions[j]
		return a.ID+"/"+a.Source.Path < b.ID+"/"+b.Source.Path
	})
	bindings, err := extractDecoderBindings(files, result.Functions, packageNames)
	if err != nil {
		return result, err
	}
	result.DecoderBindings = bindings
	reports, err := extractReportedBuildTags(files, packageNames)
	if err != nil {
		return result, err
	}
	result.ReportedBuildTags = reports
	return result, nil
}

func run(input io.Reader, output io.Writer) error {
	content, err := io.ReadAll(io.LimitReader(input, maxInputBytes+1))
	if err != nil || len(content) > maxInputBytes {
		return fmt.Errorf("source inventory exceeds input limit or cannot be read")
	}
	var request sourceRequest
	decoder := json.NewDecoder(bytes.NewReader(content))
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(&request); err != nil {
		return fmt.Errorf("invalid source inventory input")
	}
	if err := decoder.Decode(new(any)); err != io.EOF {
		return fmt.Errorf("unexpected trailing source inventory input")
	}
	result, err := extractModule(request.Files, request.ModulePath)
	if err != nil {
		return err
	}
	return json.NewEncoder(output).Encode(result)
}

func main() {
	if err := run(os.Stdin, os.Stdout); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
