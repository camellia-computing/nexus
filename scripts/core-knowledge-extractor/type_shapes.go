package main

import (
	"go/ast"
)

// Shapes retain syntax-level type relationships; decoder behavior is separate evidence.
type typeShape struct {
	Kind      string      `json:"kind"`
	Name      string      `json:"name,omitempty"`
	Package   string      `json:"package,omitempty"`
	Element   *typeShape  `json:"element,omitempty"`
	Key       *typeShape  `json:"key,omitempty"`
	Target    *typeShape  `json:"target,omitempty"`
	Arguments []typeShape `json:"arguments,omitempty"`
}

func shapePointer(shape typeShape) *typeShape { return &shape }

func describeType(expr ast.Expr, packagePath string, imports map[string]string) typeShape {
	switch item := expr.(type) {
	case *ast.ParenExpr:
		return describeType(item.X, packagePath, imports)
	case *ast.Ident:
		switch item.Name {
		case "bool", "string", "byte", "rune", "int", "int8", "int16", "int32", "int64",
			"uint", "uint8", "uint16", "uint32", "uint64", "uintptr", "float32", "float64",
			"complex64", "complex128":
			return typeShape{Kind: "builtin", Name: item.Name}
		case "any":
			return typeShape{Kind: "dynamic"}
		default:
			return typeShape{Kind: "named", Package: packagePath, Name: item.Name}
		}
	case *ast.SelectorExpr:
		if alias, ok := item.X.(*ast.Ident); ok {
			if imported, ok := imports[alias.Name]; ok {
				return typeShape{Kind: "named", Package: imported, Name: item.Sel.Name}
			}
		}
		return typeShape{Kind: "opaque", Name: "selector"}
	case *ast.StarExpr:
		return typeShape{Kind: "pointer", Element: shapePointer(describeType(item.X, packagePath, imports))}
	case *ast.ArrayType:
		return typeShape{Kind: "sequence", Element: shapePointer(describeType(item.Elt, packagePath, imports))}
	case *ast.MapType:
		return typeShape{Kind: "mapping", Key: shapePointer(describeType(item.Key, packagePath, imports)),
			Element: shapePointer(describeType(item.Value, packagePath, imports))}
	case *ast.StructType:
		// Named struct fields are inventoried with tags and source references on the declaration.
		return typeShape{Kind: "structure"}
	case *ast.InterfaceType:
		return typeShape{Kind: "dynamic"}
	case *ast.IndexExpr:
		return typeShape{Kind: "generic", Target: shapePointer(describeType(item.X, packagePath, imports)),
			Arguments: []typeShape{describeType(item.Index, packagePath, imports)}}
	case *ast.IndexListExpr:
		arguments := make([]typeShape, 0, len(item.Indices))
		for _, argument := range item.Indices {
			arguments = append(arguments, describeType(argument, packagePath, imports))
		}
		return typeShape{Kind: "generic", Target: shapePointer(describeType(item.X, packagePath, imports)), Arguments: arguments}
	case *ast.FuncType:
		return typeShape{Kind: "opaque", Name: "function"}
	case *ast.ChanType:
		return typeShape{Kind: "opaque", Name: "channel"}
	default:
		return typeShape{Kind: "opaque", Name: "expression"}
	}
}
