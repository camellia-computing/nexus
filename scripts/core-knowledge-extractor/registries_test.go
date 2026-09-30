package main

import (
	"reflect"
	"strings"
	"testing"
)

func TestXrayDecoderAliasesAndSettingsPath(t *testing.T) {
	source := `package conf
var outboundConfigLoader = NewJSONConfigLoader(ConfigCreatorCache{
 "direct": func() interface{} { return new(FreedomConfig) },
 "freedom": func() interface{} { return &FreedomConfig{} },
}, "protocol", "settings")
type FreedomConfig struct { Domain string }
func (c *FreedomConfig) Build() (any, error) { return c, nil }
`
	loader := `package conf
import "strings"
func (v *JSONConfigLoader) LoadWithID(raw []byte, id string) (any, error) {
 id = strings.ToLower(id)
 config, err := v.cache.CreateConfig(id)
 return config, err
}
`
	result, err := extract([]sourceFile{{"infra/conf/xray.go", source}, {"infra/conf/loader.go", loader}})
	if err != nil || len(result.DecoderBindings) != 2 {
		t.Fatalf("bindings missing: %+v / %v", result.DecoderBindings, err)
	}
	for _, binding := range result.DecoderBindings {
		if binding.Declaration != "infra/conf#FreedomConfig" || binding.Discriminator != "protocol" || binding.DiscriminatorComparison != "asciiCaseInsensitive" || !reflect.DeepEqual(binding.OptionsPath, []string{"settings"}) || len(binding.Evidence) != 3 || binding.Constructor != "infra/conf#FreedomConfig.Build" {
			t.Fatalf("incorrect dispatch evidence: %+v", binding)
		}
	}
	if _, err := extract([]sourceFile{{"infra/conf/xray.go", strings.ReplaceAll(source, "return new(FreedomConfig)", "return chooseConfig()")}, {"infra/conf/loader.go", loader}}); err == nil {
		t.Fatal("unresolved dynamic constructor must require review")
	}
	for _, changed := range []string{
		strings.ReplaceAll(loader, "strings.ToLower(id)", "strings.ToUpper(id)"),
		strings.ReplaceAll(loader, "CreateConfig(id)", "CreateConfig(other)"),
		strings.ReplaceAll(loader, `"strings"`, `"example.test/strings"`),
	} {
		if _, err := extract([]sourceFile{{"infra/conf/xray.go", source}, {"infra/conf/loader.go", changed}}); err == nil {
			t.Fatal("changed discriminator semantics must require review")
		}
	}
}

func TestMihomoProxyDecoderIsNotInferredFromTypeNames(t *testing.T) {
	structure := sourceFile{"common/structure/structure.go", `package structure
import "strings"
var DefaultKeyReplacer = strings.NewReplacer("_", "-")
func (d *Decoder) Decode(src map[string]any, dst any) error {
 return d.decode("", src, dst)
}
func (d *Decoder) decodeStructFromMap(name string, dataVal, val any) error {
 if fieldType.Anonymous { flatten(fieldType) }
 tag := fieldType.Tag.Get(d.option.TagName)
 key := d.option.KeyReplacer.Replace(tag)
 if strings.EqualFold(key, fieldName) { decode() }
 return nil
}
`}
	source := `package adapter
import "github.com/metacubex/mihomo/adapter/outbound"
import "github.com/metacubex/mihomo/common/structure"
func ParseProxy(mapping map[string]any) (any,error) {
 decoder := structure.NewDecoder(structure.Option{TagName: "proxy", WeaklyTypedInput: true})
 proxyType, exists := mapping["type"].(string)
 if !exists { return nil, missingType }
 var proxy any
 var err error
 switch proxyType {
 case "hysteria2", "hy2":
  option := &outbound.Hysteria2Option{}
  err = decoder.Decode(mapping, option)
  if err != nil { break }
  proxy, err = outbound.NewHysteria2(*option)
 default: return nil, badType
 }
 return proxy,err
}`
	constructor := sourceFile{"adapter/outbound/hysteria2.go", "package outbound\ntype Hysteria2Option struct { Timeout int }\nfunc NewHysteria2(o Hysteria2Option) (any,error) { return o,nil }"}
	result, err := extract([]sourceFile{{"adapter/parser.go", source}, constructor, structure})
	if err != nil || len(result.DecoderBindings) != 2 {
		t.Fatalf("proxy bindings missing: %+v / %v", result.DecoderBindings, err)
	}
	for _, binding := range result.DecoderBindings {
		if binding.Encoding != "proxy" || binding.Declaration != "adapter/outbound#Hysteria2Option" || len(binding.OptionsPath) != 0 || len(binding.Evidence) != 4 || binding.ObjectLayout.KeyComparison != "asciiCaseInsensitive" {
			t.Fatalf("proxy decoding semantics lost: %+v", binding)
		}
	}
	stub := sourceFile{"adapter/outbound/hysteria2_stub.go", "//go:build no_quic\npackage outbound\nimport \"fmt\"\nfunc NewHysteria2(o Hysteria2Option) (any,error) { return nil,fmt.Errorf(\"unavailable\") }"}
	branches, err := extract([]sourceFile{{"adapter/parser.go", source}, constructor, stub, structure})
	if err != nil {
		t.Fatal(err)
	}
	for _, binding := range branches.DecoderBindings {
		if strings.Contains(binding.BuildConstraint, "no_quic") {
			if binding.ConstructorStatus != "rejected" || binding.Evidence[len(binding.Evidence)-1].Source.Path != stub.Path {
				t.Fatalf("rejecting constructor became a capability: %+v", binding)
			}
		} else if binding.ConstructorStatus != "registered" {
			t.Fatalf("usable constructor was lost: %+v", binding)
		}
	}
	for _, altered := range []string{
		strings.ReplaceAll(source, `TagName: "proxy"`, `TagName: "json"`),
		strings.ReplaceAll(source, `mapping["type"]`, `mapping["name"]`),
		strings.ReplaceAll(source, `NewHysteria2(*option)`, `NewHysteria2(*differentOption)`),
		strings.ReplaceAll(source, `decoder.Decode(mapping, option)`, `decoder.Decode(other, option)`),
	} {
		if _, err := extract([]sourceFile{{"adapter/parser.go", altered}, constructor, structure}); err == nil {
			t.Fatal("changed proxy dispatch must require review")
		}
	}
	shared := strings.ReplaceAll(source, "WeaklyTypedInput: true", "WeaklyTypedInput: true, KeyReplacer: structure.DefaultKeyReplacer")
	shared = strings.ReplaceAll(shared, "return proxy,err", `if muxMapping, exists := mapping["smux"].(map[string]any); exists {
 muxOption := &outbound.SingMuxOption{}
 err = decoder.Decode(muxMapping, muxOption)
}
return proxy,err`)
	result, err = extract([]sourceFile{{"adapter/parser.go", shared}, constructor, structure})
	if err != nil {
		t.Fatal(err)
	}
	layout := result.DecoderBindings[0].ObjectLayout
	if layout.KeyComparison != "asciiCaseInsensitiveUnderscore" || len(layout.SharedOptions) != 1 ||
		layout.SharedOptions[0].Declaration != "adapter/outbound#SingMuxOption" || !reflect.DeepEqual(layout.SharedOptions[0].Path, []string{"smux"}) {
		t.Fatalf("shared decoding lost: %+v", layout)
	}
	direct := strings.ReplaceAll(structure.Content, "if fieldType.Anonymous { flatten(fieldType) }", "")
	direct = strings.ReplaceAll(direct, `return d.decode("", src, dst)`, `if fieldType.Anonymous { flatten(fieldType) }
 tag := fieldType.Tag.Get(d.option.TagName)
 key := d.option.KeyReplacer.Replace(tag)
 if strings.EqualFold(key, fieldName) { decode() }
 return nil`)
	result, err = extract([]sourceFile{{"adapter/parser.go", shared}, constructor, {structure.Path, direct}})
	if err != nil {
		t.Fatal(err)
	}
	layout = result.DecoderBindings[0].ObjectLayout
	if layout.RootFieldNames || layout.NestedEmbedded {
		t.Fatalf("entry and nested structure semantics were conflated: %+v", layout)
	}
	for _, altered := range []string{
		strings.ReplaceAll(shared, "decoder.Decode(muxMapping, muxOption)", "decoder.Decode(other, muxOption)"),
		strings.ReplaceAll(shared, "return proxy,err", `unused := mapping["unreviewed"]; return proxy,unused`),
	} {
		if _, err := extract([]sourceFile{{"adapter/parser.go", altered}, constructor, structure}); err == nil {
			t.Fatal("shared decoder change must require review")
		}
	}
	for _, altered := range []string{
		strings.ReplaceAll(structure.Content, "EqualFold", "Contains"),
		strings.ReplaceAll(structure.Content, `NewReplacer("_", "-")`, `NewReplacer(".", "-")`),
		strings.ReplaceAll(structure.Content, "*Decoder)", "*OtherDecoder)"),
	} {
		if _, err := extract([]sourceFile{{"adapter/parser.go", shared}, constructor, {structure.Path, altered}}); err == nil {
			t.Fatal("field decoding change must require review")
		}
	}
}

func TestSingBoxRegistriesPreserveBuildBranchesAndRejectingConstructors(t *testing.T) {
	files := []sourceFile{
		{"include/registry.go", `package include
import "github.com/sagernet/sing-box/adapter/inbound"
import "github.com/sagernet/sing-box/adapter/outbound"
func InboundRegistry() any { registry := inbound.NewRegistry(); return registry }
func OutboundRegistry() any { registry := outbound.NewRegistry(); registerQUIC(registry); return registry }
`},
		{"include/quic.go", `//go:build with_quic
package include
import "github.com/sagernet/sing-box/protocol/tuic"
func registerQUIC(registry any) { tuic.RegisterOutbound(registry) }
`},
		{"include/quic_stub.go", `//go:build !with_quic
package include
import "github.com/sagernet/sing-box/adapter/outbound"
import "github.com/sagernet/sing-box/option"
import C "github.com/sagernet/sing-box/constant"
func registerQUIC(registry any) { outbound.Register[option.TUICOutboundOptions](registry, C.TypeTUIC, func() (any,error) { return nil, missingQUIC }) }
`},
		{"protocol/tuic/outbound.go", `package tuic
import "github.com/sagernet/sing-box/adapter/outbound"
import "github.com/sagernet/sing-box/option"
import C "github.com/sagernet/sing-box/constant"
func RegisterOutbound(registry any) { outbound.Register[option.TUICOutboundOptions](registry, C.TypeTUIC, NewOutbound) }
func NewOutbound(o option.TUICOutboundOptions) (any,error) { return o,nil }
`},
		{"constant/types.go", "package constant\nconst TypeTUIC = \"tuic\""},
		{"option/tuic.go", "package option\ntype TUICOutboundOptions struct{}"},
	}
	result, err := extract(files)
	if err != nil || len(result.DecoderBindings) != 2 {
		t.Fatalf("build variants missing: %+v / %v", result.DecoderBindings, err)
	}
	for _, binding := range result.DecoderBindings {
		if binding.Value != "tuic" || binding.Declaration != "option#TUICOutboundOptions" {
			t.Fatalf("wrong registration: %+v", binding)
		}
		if binding.ConstructorStatus == "rejected" {
			if !strings.Contains(binding.BuildConstraint, "!with_quic") || binding.Constructor != "" {
				t.Fatalf("rejecting stub reported as usable: %+v", binding)
			}
		} else if !strings.Contains(binding.BuildConstraint, "with_quic") || strings.Contains(binding.BuildConstraint, "!with_quic") || binding.Constructor != "protocol/tuic#NewOutbound" || len(binding.Evidence) != 4 {
			t.Fatalf("constructor evidence missing: %+v", binding)
		}
	}
	reversed := append([]sourceFile{}, files...)
	for i, j := 0, len(reversed)-1; i < j; i, j = i+1, j-1 {
		reversed[i], reversed[j] = reversed[j], reversed[i]
	}
	again, err := extract(reversed)
	if err != nil || !reflect.DeepEqual(result, again) {
		t.Fatal("registry evidence depends on source order")
	}
	files[1].Content = strings.ReplaceAll(files[1].Content, "tuic.RegisterOutbound(registry)", "if enabled { tuic.RegisterOutbound(registry) }")
	if _, err := extract(files); err == nil {
		t.Fatal("runtime-conditional registration cannot become an unconditional capability")
	}
}
