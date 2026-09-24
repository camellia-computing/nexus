"""Stable window and evidence contract tests."""

import unittest
import json
from pathlib import Path
import tempfile
from copy import deepcopy
from unittest.mock import patch

from core_knowledge import stable_version, stable_window, structural_changes, behavior_changes, decoder_changes, reviewed_rules, discover, type_reference_coverage, dependency_version, attribute_inventory, resolved_dependency_commit, dependency_inventory, build, apply_reviewed_layouts, reviewed_value_decoders


def release(tag, *, prerelease=False, draft=False):
    return {"tag_name": tag, "prerelease": prerelease, "draft": draft}


class StableWindowTests(unittest.TestCase):
    def test_latest_anchors_two_families_and_keeps_each_patch(self):
        latest = release("v1.14.0")
        releases = [release("v1.15.0-beta.1", prerelease=True), latest, release("v1.13.1"), release("v1.13.0"), release("v1.12.9")]
        families, selected = stable_window(releases, latest)
        self.assertEqual(families, [(1, 14), (1, 13)])
        self.assertEqual([item["tag_name"] for item in selected], ["v1.13.0", "v1.13.1", "v1.14.0"])

    def test_calendar_uses_actual_months_and_crosses_year_boundary(self):
        latest = release("v26.1.23")
        families, selected = stable_window([latest, release("v25.11.6"), release("v25.11.1"), release("v25.10.5"), release("v26.2.1", prerelease=True)], latest)
        self.assertEqual(families, [(26, 1), (25, 11)])
        self.assertEqual(len(selected), 3)

    def test_rejects_alias_and_ambiguous_identity(self):
        latest = release("v26.3.27")
        _, selected = stable_window([latest, release("v1.260327.0"), release("v26.2.6")], latest)
        self.assertNotIn("v1.260327.0", [item["tag_name"] for item in selected])
        with self.assertRaises(ValueError):
            stable_window([latest, release("26.3.27"), release("v26.2.6")], latest)

    def test_rejects_incomplete_or_unstable_anchor(self):
        for releases, latest in [([release("v1.14.0")], release("v1.14.0")), ([release("v1.14.0"), release("v1.13.0")], release("v1.15.0")), ([release("v1.14.0"), release("v1.13.0")], release("v1.14.0", prerelease=True))]:
            with self.assertRaises(ValueError):
                stable_window(releases, latest)

    def test_version_parser_rejects_nonrelease_text(self):
        for text in ["sing-box version 1.14.0", "v1.14.0-beta.1", "v01.14.0", "v1.14.0+custom", "v1.14", "1.14.0 trailing"]:
            self.assertIsNone(stable_version(text))

    def test_month_policy_excludes_module_tags_and_invalid_months(self):
        latest = release("v26.3.27")
        families, selected = stable_window([latest, release("v26.2.6"), release("v26.0.0"), release("v26.13.1"), release("v1.260327.0")], latest, "releaseMonth")
        self.assertEqual(families, [(26, 3), (26, 2)])
        self.assertEqual(len(selected), 2)
        with self.assertRaises(ValueError):
            stable_window([release("v26.13.1"), latest], release("v26.13.1"), "releaseMonth")

    def test_discovery_does_not_stop_at_a_recent_backport(self):
        latest = release("v1.14.0")
        first_page = [latest, release("v1.13.2"), release("v1.12.30"), *[release(f"v1.15.0-beta.{index}", prerelease=True) for index in range(97)]]
        second_page = [release("v1.13.1"), release("v1.13.0")]
        with patch("core_knowledge.github", side_effect=[latest, first_page, second_page]) as api:
            _, _, selected = discover({"repository": "fixture/core", "familyPolicy": "majorMinor"})
        self.assertEqual(api.call_count, 3)
        self.assertEqual([item["tag_name"] for item in selected], ["v1.13.0", "v1.13.1", "v1.13.2", "v1.14.0"])


class SourceReviewTests(unittest.TestCase):
    def test_value_decoder_review_binds_forms_fields_imports_and_each_branch(self):
        obj = self.declaration()
        obj.update(shape={"kind": "structure"}, methods=[])
        custom = {**obj, "id": "option#Custom", "methods": ["UnmarshalJSON"]}
        source = {"modulePath": "example.test/module", "path": "codec/decode.go", "symbol": "Decode"}
        inventory = {"declarations": [obj, custom], "functions": [
            {"id": "codec#Decode", "bodyHash": "a", "buildConstraint": "(!alternate)", "imports": {"json": "example.test/json"}, "source": source},
            {"id": "codec#Decode", "bodyHash": "a", "buildConstraint": "(alternate)", "imports": {"json": "encoding/json"}, "source": source}]}
        recipe = {"program": "fixture", "declaration": custom["id"], "objectDeclaration": obj["id"], "encoding": "json", "scalarForms": ["boolean", "null"], "keyComparison": "asciiCaseInsensitive",
                  "objectFields": [{key: f[key] for key in ("name", "type", "embedded", "tags")} for f in obj["fields"]], "layoutEvidence": [],
                  "evidence": [{"function": "codec#Decode", "branches": [
                      {"supported": True, "bodyHashes": ["a"], "buildConstraints": ["(!alternate)"], "imports": {"json": "example.test/json"}},
                      {"supported": False, "bodyHashes": ["a"], "buildConstraints": ["(alternate)"], "imports": {"json": "encoding/json"}}]}]}
        result = reviewed_value_decoders("fixture", inventory, [recipe], [])
        self.assertEqual(result[0]["buildConstraint"], "(!alternate)")
        self.assertEqual(result[0]["scalarForms"], ["boolean", "null"])
        self.assertEqual(len(result[0]["evidence"]), 2)
        for mutation in range(8):
            invalid = deepcopy(inventory)
            if mutation == 0: invalid["functions"][0]["bodyHash"] = "changed"
            elif mutation == 1: invalid["functions"][1]["imports"]["json"] = "example.test/unreviewed"
            elif mutation == 2: invalid["declarations"][0]["fields"][0]["tags"] = {"json": "other"}
            elif mutation == 3: invalid["functions"].append(deepcopy(invalid["functions"][0]))
            elif mutation == 4: invalid["functions"][0]["buildConstraint"] = ""
            elif mutation == 5: invalid["declarations"][0]["methods"] = ["UnmarshalJSON"]
            elif mutation == 6: invalid["functions"] = []
            else: invalid["declarations"].append(deepcopy(invalid["declarations"][0]))
            with self.assertRaises(ValueError, msg=f"mutation {mutation}"):
                reviewed_value_decoders("fixture", invalid, [recipe], [])

    def test_flat_decoder_review_binds_fields_imports_behavior_and_build_branch(self):
        declaration = self.declaration()
        source = {"modulePath": "example.test/module", "path": "codec/decode.go", "symbol": "Decode"}
        functions = [
            {"id": "codec#Decode", "bodyHash": "a", "buildConstraint": "(!without_contextjson)", "source": source, "imports": {"json": "example.test/codec"}},
            {"id": "codec#Decode", "bodyHash": "b", "buildConstraint": "(without_contextjson)", "source": source, "imports": {}},
            {"id": "option#Parse", "bodyHash": "c", "buildConstraint": "", "source": source, "imports": {}},
        ]
        inventory = {"functions": functions, "declarations": [declaration], "decoderBindings": [
            {"path": ["outbounds", "*"], "optionsPath": [], "encoding": "json", "objectLayout": None, "evidence": []}]}
        recipe = {"program": "fixture", "envelopeFields": [{key: field[key] for key in ("name", "type", "embedded", "tags")} for field in declaration["fields"]],
                  "roots": [{"collection": "outbounds", "envelope": declaration["id"], "evidence": {"function": "option#Parse", "bodyHashes": ["c"]}}],
                  "evidence": [{"function": "codec#Decode", "bodyHashes": ["a"], "unsupportedBodyHashes": ["b"], "buildConstraints": ["", "(!without_contextjson)"], "imports": {"json": "example.test/codec"}}]}
        result = deepcopy(inventory)
        apply_reviewed_layouts("fixture", result, [recipe])
        binding = result["decoderBindings"][0]
        self.assertEqual(binding["objectLayout"]["buildConstraint"], "(!without_contextjson)")
        self.assertEqual(binding["objectLayout"]["envelopeKeyComparison"], "exact")
        self.assertEqual(binding["objectLayout"]["keyComparison"], "asciiCaseInsensitive")
        self.assertEqual(len(binding["evidence"]), 2)
        for mutation in range(6):
            invalid = deepcopy(inventory)
            if mutation == 0: invalid["functions"][0]["bodyHash"] = "changed"
            elif mutation == 1: invalid["functions"][0]["imports"]["json"] = "example.test/another"
            elif mutation == 2: invalid["declarations"][0]["fields"][0]["tags"] = {"json":"changed"}
            elif mutation == 3: invalid["functions"].append(deepcopy(invalid["functions"][0]))
            elif mutation == 4: invalid["functions"][0]["buildConstraint"] = ""
            else: invalid["functions"] = []
            with self.assertRaises(ValueError, msg=f"mutation {mutation}"):
                apply_reviewed_layouts("fixture", invalid, [recipe])

    def test_source_rebuild_cannot_publish_a_mixed_producer_revision(self):
        with tempfile.TemporaryDirectory() as directory, patch("core_knowledge.PROGRAMS", ()), patch("core_knowledge.run", return_value=b""), patch("core_knowledge.producer_hash", side_effect=["a" * 64, "b" * 64]):
            with self.assertRaisesRegex(ValueError, "producer changed"):
                build(Path(directory))

    def test_behavior_review_tracks_import_rebinding_and_rejects_ambiguous_symbols(self):
        before = {"id": "codec#Decode", "bodyHash": "a" * 64, "buildConstraint": "!alternate",
                  "source": {"path": "codec/decode.go", "symbol": "Decode"}, "imports": {"json": "example.test/strict"}}
        after = {**before, "imports": {"json": "example.test/permissive"}}
        changes = behavior_changes([before], [after])
        self.assertEqual(len(changes), 1)
        self.assertEqual(changes[0]["beforeHash"], changes[0]["afterHash"])
        self.assertNotEqual(changes[0]["beforeImports"], changes[0]["afterImports"])
        with self.assertRaises(ValueError):
            behavior_changes([before, before], [after])

    def test_dependency_git_tag_must_match_module_origin_even_when_discovered_on_a_branch(self):
        descriptor = {"modulePath": "github.com/example/dependency", "repository": "Example/dependency", "prefixes": ["option/"]}
        version = "v0.9.0"
        resolved = {"Path": descriptor["modulePath"], "Version": version,
                    "Sum": "h1:" + "a" * 43 + "=", "GoModSum": "h1:" + "b" * 43 + "=",
                    "Origin": {"VCS": "git", "URL": "https://github.com/example/dependency", "Ref": "refs/heads/main", "Hash": "c" * 40}}
        calls = []
        def run(args, **kwargs):
            calls.append(args)
            if args[:3] == ["go", "mod", "download"]:
                return json.dumps(resolved).encode()
            if "rev-parse" in args:
                return ("d" * 40).encode()
            return b""
        with tempfile.TemporaryDirectory() as directory, patch("core_knowledge.run", side_effect=run):
            with self.assertRaisesRegex(ValueError, "Git source differs"):
                dependency_inventory(Path(directory), {"Require": [{"Path": descriptor["modulePath"], "Version": version}]}, descriptor, Path(directory) / "extractor")
        fetched = next(args for args in calls if "fetch" in args)
        self.assertEqual(fetched[-1], "refs/tags/v0.9.0:refs/tags/v0.9.0")
        self.assertFalse(any("refs/heads/main" in args for args in calls))

    def test_dependency_resolution_binds_repository_module_version_and_full_commit(self):
        descriptor = {"modulePath": "github.com/example/dependency", "repository": "Example/dependency"}
        version = "v0.9.0-beta.4"
        resolved = {"Path": descriptor["modulePath"], "Version": version,
                    "Sum": "h1:" + "a" * 43 + "=", "GoModSum": "h1:" + "b" * 43 + "=",
                    "Origin": {"VCS": "git", "URL": "https://github.com/example/dependency", "Ref": "refs/tags/" + version, "Hash": "c" * 40}}
        self.assertEqual(resolved_dependency_commit(resolved, descriptor, version), "c" * 40)
        for invalid in [
            {**resolved, "Path": "github.com/unreviewed/dependency"},
            {**resolved, "Version": "v0.9.0"},
            {**resolved, "Sum": ""},
            {**resolved, "Origin": None},
            *[{**resolved, "Origin": {**resolved["Origin"], key: value}} for key, value in [
                ("Hash", "c" * 12), ("URL", "https://github.com/example/dependency.evil"),
                ("VCS", "other")]],
        ]:
            with self.assertRaises(ValueError):
                resolved_dependency_commit(invalid, descriptor, version)
        pseudo = "v0.9.0-beta.4.0.20260102030405-" + "c" * 12
        self.assertEqual(resolved_dependency_commit({**resolved, "Origin": {**resolved["Origin"], "Ref": "refs/heads/main"}}, descriptor, version), "c" * 40)
        pinned = {**resolved, "Version": pseudo, "Origin": {**resolved["Origin"], "Ref": ""}}
        self.assertEqual(resolved_dependency_commit(pinned, descriptor, pseudo), "c" * 40)
        with self.assertRaises(ValueError):
            resolved_dependency_commit({**pinned, "Origin": {**pinned["Origin"], "Hash": "d" * 40}}, descriptor, pseudo)

    def test_dependency_version_requires_exact_unreplaced_requirement(self):
        module = "example.org/dependency"
        manifest = {"Require": [{"Path": module, "Version": "v0.9.0-beta.4"}]}
        self.assertEqual(dependency_version(manifest, module), "v0.9.0-beta.4")
        for invalid in [
            {}, {"Require": manifest["Require"] * 2},
            {"Require": [{"Path": module, "Version": "main"}]},
            {"Require": [{"Path": module, "Version": "../private"}]},
            {**manifest, "Replace": [{"Old": {"Path": module}, "New": {"Path": "../local"}}]},
            {**manifest, "Exclude": manifest["Require"]},
        ]:
            with self.assertRaises(ValueError):
                dependency_version(invalid, module)
        self.assertEqual(dependency_version({**manifest, "Replace": None, "Exclude": None}, module), "v0.9.0-beta.4")

    def test_dependency_namespace_keeps_local_imported_and_source_identity_distinct(self):
        module = "example.org/dependency"
        declaration = self.declaration()
        declaration["shape"] = {"kind": "structure"}
        declaration["fields"][0]["shape"] = {"kind": "generic", "target": {"kind": "named", "package": "option", "name": "Options"}, "arguments": [
            {"kind": "named", "package": "example.org/other/option", "name": "Options"},
            {"kind": "named", "package": "time", "name": "Duration"},
        ]}
        inventory = {"declarations": [declaration], "functions": [{"id": "option#Options.Build", "source": {"path": "option/options.go", "symbol": "Options.Build"}}]}
        result = attribute_inventory(inventory, module, dependency=True)
        entry = result["declarations"][0]
        self.assertEqual(entry["id"], module + "/option#Options")
        self.assertEqual(entry["package"], module + "/option")
        self.assertEqual(entry["source"]["modulePath"], module)
        self.assertEqual(entry["source"]["path"], "option/options.go")
        shape = entry["fields"][0]["shape"]
        self.assertEqual(shape["target"]["package"], module + "/option")
        self.assertEqual(shape["arguments"][0]["package"], "example.org/other/option")
        self.assertEqual(shape["arguments"][1]["package"], "time")
        self.assertEqual(result["functions"][0]["id"], module + "/option#Options.Build")
        self.assertEqual(type_reference_coverage([entry], "example.org/core")["resolvedNamedReferences"], 1)

    def declaration(self, identifier="option#Options", path="option/options.go"):
        return {"id": identifier, "name": "Options", "package": "option", "type": "struct", "alias": False,
                "buildConstraint": "", "imports": {}, "methods": [], "source": {"path": path, "symbol": "Options"},
                "fields": [{"name": "Timeout", "type": "int", "embedded": False, "tags": {"proxy": "handshake-timeout"}, "annotations": {}, "source": {"path": path, "symbol": "Options.Timeout"}}]}

    def test_file_move_does_not_report_field_removal(self):
        change = structural_changes([self.declaration()], [self.declaration(path="option/client.go")])
        self.assertFalse(any(change.values()))
        change = structural_changes([self.declaration()], [self.declaration("client#Options", "client/options.go")])
        self.assertEqual(len(change["relocated"]), 1)
        self.assertEqual(change["removed"], [])
        self.assertEqual(change["added"], [])

    def test_type_and_wire_tag_changes_require_review(self):
        a, b = self.declaration(), self.declaration()
        b["fields"][0]["type"] = "string"
        b["fields"][0]["tags"]["proxy"] = "timeout"
        change = structural_changes([a], [b])
        self.assertEqual(change["changed"][0]["changedFields"], ["Timeout"])
        self.assertEqual(change["changed"][0]["removedFields"], [])

    def test_ambiguous_declaration_is_not_silently_collapsed(self):
        with self.assertRaises(ValueError):
            structural_changes([self.declaration(), self.declaration(path="option/other.go")], [])

    def test_reviewed_rule_is_invalidated_by_behavior_change(self):
        fn = {"id": "option#Options.Build", "bodyHash": "a" * 64, "buildConstraint": "", "source": {"path": "option/options.go", "symbol": "Options.Build"}}
        field_shape = {key: value for key, value in self.declaration()["fields"][0].items() if key not in {"name", "source"}}
        rule = {"id": "timeout", "program": "fixture", "declaration": "option#Options", "field": "Timeout", "evidence": {"function": fn["id"], "bodyHashes": [fn["bodyHash"]], "fieldShape": field_shape}}
        inventory = {"declarations": [self.declaration()], "functions": [fn]}
        self.assertEqual(len(reviewed_rules("fixture", inventory, [rule])), 1)
        updated = {**fn, "bodyHash": "b" * 64}
        self.assertEqual(len(behavior_changes([fn], [updated])), 1)
        self.assertEqual(behavior_changes([fn], [fn]), [])
        inventory["functions"] = [updated]
        with self.assertRaises(ValueError):
            reviewed_rules("fixture", inventory, [rule])
        inventory["functions"] = [fn]
        inventory["declarations"][0]["fields"][0]["tags"]["proxy"] = "different-path"
        with self.assertRaises(ValueError):
            reviewed_rules("fixture", inventory, [rule])

    def test_type_reference_coverage_keeps_unresolved_dependencies_explicit(self):
        declarations = [{"id": "option#Options", "shape": {"kind": "structure"}, "fields": [
            {"shape": {"kind": "mapping", "key": {"kind": "builtin", "name": "string"},
                       "element": {"kind": "sequence", "element": {"kind": "named", "package": "example.org/core/option", "name": "Item"}}}},
            {"shape": {"kind": "generic", "target": {"kind": "named", "package": "example.org/dependency", "name": "List"},
                       "arguments": [{"kind": "named", "package": "option", "name": "Item"}]}}
        ]}, {"id": "option#Item", "shape": {"kind": "structure"}, "fields": []}]
        result = type_reference_coverage(declarations, "example.org/core")
        self.assertEqual(result["namedReferences"], 2)
        self.assertEqual(result["resolvedNamedReferences"], 1)
        self.assertEqual(result["unresolvedNamedReferences"], ["example.org/dependency#List"])

    def test_decoder_change_preserves_constructor_and_branch_semantics(self):
        binding = {"path": ["outbounds", "*"], "discriminator": "type", "discriminatorComparison": "exact", "value": "example", "buildConstraint": "with_feature", "declaration": "option#Example", "optionsPath": [], "encoding": "json", "objectLayout": None, "constructor": "protocol/example#New", "constructorStatus": "registered", "evidence": [{"bodyHash": "a" * 64}]}
        updated = {**binding, "constructorStatus": "rejected", "constructor": "", "evidence": [{"bodyHash": "b" * 64}]}
        change = decoder_changes([binding], [updated])
        self.assertEqual(len(change), 1)
        self.assertEqual(change[0]["before"]["constructorStatus"], "registered")
        self.assertEqual(change[0]["after"]["constructorStatus"], "rejected")
        self.assertEqual(decoder_changes([binding], [binding]), [])
        with self.assertRaises(ValueError):
            decoder_changes([binding, binding], [updated])
        updated = {**binding, "objectLayout": {
            "buildConstraint": "",
            "envelopeDeclarations": ["option#Envelope"], "envelopeKeyComparison": "exact", "sharedOptions": [],
            "keyComparison": "asciiCaseInsensitive", "rootFieldNames": True,
            "nestedEmbedded": True
        }}
        change = decoder_changes([binding], [updated])
        self.assertIsNone(change[0]["before"]["objectLayout"])
        self.assertEqual(change[0]["after"]["objectLayout"], updated["objectLayout"])


if __name__ == "__main__":
    unittest.main()
