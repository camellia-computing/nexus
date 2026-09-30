#!/usr/bin/env python3
"""Generate source-backed stable release knowledge; checks are offline by default."""

from __future__ import annotations

import argparse
from collections import Counter
from copy import deepcopy
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
ARTIFACT = ROOT / "crates/camellia-nexus-core/core-knowledge.json"
REPORT = ROOT / "docs/core-knowledge-coverage.json"
CHANGES = ROOT / "docs/core-knowledge-changes.json"
RULES = ROOT / "scripts/core-knowledge-rules.json"
PROGRAMS = (
    {"program":"singBox","repository":"SagerNet/sing-box","familyPolicy":"majorMinor","prefixes":["option/","include/","constant/","protocol/","log/"],"roots":["option#Options"],"outboundCollection":"outbounds","shareProtocols":{"proxy.outbound.vless":"vless","proxy.outbound.shadowsocks":"shadowsocks","proxy.outbound.hysteria2":"hysteria2","proxy.outbound.tuicV5":"tuic"}},
    {"program":"mihomo","repository":"MetaCubeX/mihomo","familyPolicy":"majorMinor","prefixes":["config/","adapter/","listener/","constant/","rules/","log/","common/structure/","main.go"],"roots":["config#RawConfig"],"outboundCollection":"proxies","shareProtocols":{"proxy.outbound.vless":"vless","proxy.outbound.shadowsocks":"ss","proxy.outbound.hysteria2":"hysteria2","proxy.outbound.tuicV5":"tuic"}},
    {"program":"xray","repository":"XTLS/Xray-core","familyPolicy":"releaseMonth","prefixes":["infra/conf/","transport/internet/sockopt"],"roots":["infra/conf#Config"],"outboundCollection":"outbounds","shareProtocols":{"proxy.outbound.vless":"vless","proxy.outbound.shadowsocks":"shadowsocks","proxy.outbound.hysteria2":"hysteria","proxy.outbound.tuicV5":"tuic"}},
)
STABLE_TAG = re.compile(r"^v?(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$")
SHA = re.compile(r"^[0-9a-f]{40}$")
DEPENDENCIES = {
    "singBox": [{"modulePath": "github.com/sagernet/sing", "repository": "SagerNet/sing",
                 "prefixes": ["common/json/", "common/x/linkedhashmap/"]},
                {"modulePath": "github.com/sagernet/sing-tun", "repository": "SagerNet/sing-tun",
                 "prefixes": ["redirect", "tun", "stack"]}],
}


def canonical(value: Any) -> bytes:
    return json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(",", ":")).encode()


def digest(value: Any) -> str:
    return hashlib.sha256(canonical(value)).hexdigest()


def producer_hash() -> str:
    inputs = [Path(__file__).resolve(), RULES, *sorted((ROOT / "scripts/core-knowledge-extractor").glob("*.go")), ROOT / "scripts/core-knowledge-extractor/go.mod"]
    return digest({path.relative_to(ROOT).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest() for path in inputs})


def stable_version(tag: str) -> tuple[int, int, int] | None:
    match = STABLE_TAG.fullmatch(tag)
    return tuple(int(part) for part in match.groups()) if match else None


def stable_window(releases: list[dict], latest: dict, policy: str = "majorMinor") -> tuple[list[tuple[int, int]], list[dict]]:
    if policy not in {"majorMinor", "releaseMonth"}:
        raise ValueError("unrecognized stable family policy")
    def version_of(tag: str) -> tuple[int, int, int] | None:
        version = stable_version(tag)
        return version if version and (policy != "releaseMonth" or 1 <= version[1] <= 12) else None
    anchor = version_of(latest.get("tag_name", ""))
    if not anchor or latest.get("draft") or latest.get("prerelease"):
        raise ValueError("official latest must be a stable release")
    stable = [release for release in releases if not release.get("draft") and not release.get("prerelease") and version_of(release.get("tag_name", ""))]
    families = sorted({version_of(release["tag_name"])[:2] for release in stable if version_of(release["tag_name"])[:2] <= anchor[:2]}, reverse=True)
    if len(families) < 2 or families[0] != anchor[:2]:
        raise ValueError("release listing does not establish two stable families")
    families = families[:2]
    selected = [release for release in stable if stable_version(release["tag_name"])[:2] in families]
    if not any(release["tag_name"] == latest["tag_name"] for release in selected):
        raise ValueError("official latest is missing from release listing")
    versions = [stable_version(release["tag_name"]) for release in selected]
    if len(set(versions)) != len(versions):
        raise ValueError("stable release versions are ambiguous")
    return families, sorted(selected, key=lambda release: stable_version(release["tag_name"]))


def run(args: list[str], *, cwd: Path | None = None, data: bytes | None = None, timeout: int = 180) -> bytes:
    result = subprocess.run(args, cwd=cwd, input=data, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=timeout)
    if result.returncode:
        # Tool output may include arbitrary source text or credential-bearing URLs.
        if Path(args[0]).name == "core-knowledge-extractor":
            raise RuntimeError(f"source extraction failed: {result.stderr[:512].decode(errors='replace').strip()}")
        raise RuntimeError(f"{Path(args[0]).name} failed with exit code {result.returncode}")
    return result.stdout


def github(path: str) -> Any:
    return json.loads(run(["gh", "api", path], timeout=45))


def discover(descriptor: dict) -> tuple[dict, list[tuple[int, int]], list[dict]]:
    repository = descriptor["repository"]
    latest = github(f"repos/{repository}/releases/latest")
    releases: list[dict] = []
    for page in range(1, 21):
        batch = github(f"repos/{repository}/releases?per_page=100&page={page}")
        releases.extend(batch)
        # Publication order is not version order: a backport can precede retained patches.
        if len(batch) < 100:
            families, selected = stable_window(releases, latest, descriptor["familyPolicy"])
            return latest, families, selected
    raise RuntimeError("release listing exceeded discovery limit")


def source_module_manifest(mirror: Path, sha: str) -> dict:
    content = run(["git", "-C", str(mirror), "show", f"{sha}:go.mod"])
    with tempfile.TemporaryDirectory(prefix="nexus-module-") as directory:
        module = Path(directory) / "go.mod"
        module.write_bytes(content)
        parsed = json.loads(run(["go", "mod", "edit", "-json", str(module)], cwd=Path(directory)))
    path = parsed.get("Module", {}).get("Path", "")
    if not path or any(part in {"", ".", ".."} for part in path.split("/")):
        raise ValueError("source module path is invalid")
    return parsed


def source_module(mirror: Path, sha: str) -> str:
    return source_module_manifest(mirror, sha)["Module"]["Path"]


def dependency_version(manifest: dict, module_path: str) -> str:
    requirements = [entry for entry in manifest.get("Require") or [] if entry["Path"] == module_path]
    if len(requirements) != 1:
        raise ValueError("reviewed dependency must have one exact module requirement")
    version = requirements[0]["Version"]
    # Resolve pinned module versions only within the reviewed repository.
    if not re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*)?", version):
        raise ValueError("dependency version requires source review")
    if any(entry["Old"]["Path"] == module_path for entry in manifest.get("Replace") or []):
        raise ValueError("dependency replacement requires source review")
    if any(entry["Path"] == module_path and entry["Version"] == version for entry in manifest.get("Exclude") or []):
        raise ValueError("reviewed dependency requirement is excluded")
    return version


def attribute_inventory(inventory: dict, module_path: str, *, dependency: bool) -> dict:
    """Bind every source reference to its module; external declaration IDs use full import paths."""
    packages = {entry["package"] for entry in inventory["declarations"]}
    def qualified(package):
        return module_path if package == "." else module_path + "/" + package
    def visit(value):
        if isinstance(value, list):
            for entry in value:
                visit(entry)
        elif isinstance(value, dict):
            if "source" in value:
                value["source"]["modulePath"] = module_path
            if dependency:
                if value.get("kind") == "named" and value["package"] in packages:
                    value["package"] = qualified(value["package"])
                if "id" in value and "#" in value["id"]:
                    package, symbol = value["id"].split("#", 1)
                    value["id"] = qualified(package) + "#" + symbol
                if "fields" in value and "package" in value:
                    value["package"] = qualified(value["package"])
                if "declaration" in value and isinstance(value["declaration"], str):
                    package, symbol = value["declaration"].split("#", 1)
                    value["declaration"] = qualified(package) + "#" + symbol
            for entry in value.values():
                visit(entry)
    visit(inventory)
    return inventory


def dependency_inventory(cache: Path, manifest: dict, descriptor: dict, extractor: Path) -> tuple[dict, dict]:
    module_path = descriptor["modulePath"]
    version = dependency_version(manifest, module_path)
    mirror = cache / ("dependency-" + hashlib.sha256(module_path.encode()).hexdigest()[:16] + ".git")
    if not mirror.exists():
        run(["git", "init", "--bare", str(mirror)])
    with tempfile.TemporaryDirectory(prefix="nexus-dependency-") as directory:
        resolved = json.loads(run(["go", "mod", "download", "-json", module_path + "@" + version], cwd=Path(directory)))
    sha = resolved_dependency_commit(resolved, descriptor, version)
    pseudo = re.search(r"[.-][0-9]{14}-[0-9a-f]{12}$", version)
    ref = sha if pseudo else "refs/tags/" + version
    destination = sha if pseudo else f"{ref}:{ref}"
    run(["git", "-C", str(mirror), "fetch", "--depth=1", f"https://github.com/{descriptor['repository']}.git", destination], timeout=600)
    git_sha = run(["git", "-C", str(mirror), "rev-parse", ref + "^{commit}"]).decode().strip()
    if git_sha != sha:
        raise ValueError("dependency Git source differs from the verified module origin")
    if not SHA.fullmatch(sha) or source_module(mirror, sha) != module_path:
        raise ValueError("dependency source identity does not match its module")
    identity = {"modulePath": module_path, "version": version, "repository": descriptor["repository"],
                "commitSha": sha, "sourceUrl": f"https://github.com/{descriptor['repository']}/tree/{sha}"}
    inventory = attribute_inventory(source_inventory(mirror, sha, descriptor["prefixes"], extractor), module_path, dependency=True)
    return identity, inventory


def resolved_dependency_commit(resolved: dict, descriptor: dict, version: str) -> str:
    origin = resolved.get("Origin") or {}
    sha = origin.get("Hash", "")
    if (resolved.get("Error") or resolved.get("Path") != descriptor["modulePath"]
            or resolved.get("Version") != version or origin.get("VCS") != "git"
            or origin.get("URL", "").removesuffix(".git").lower() != f"https://github.com/{descriptor['repository']}".lower()
            or not SHA.fullmatch(sha)
            or not all(re.fullmatch(r"h1:[A-Za-z0-9+/]{43}=", resolved.get(key, "")) for key in ("Sum", "GoModSum"))):
        raise ValueError("dependency resolver lacks exact reviewed source identity")
    pseudo = re.search(r"[.-][0-9]{14}-([0-9a-f]{12})$", version)
    if pseudo:
        if not sha.startswith(pseudo[1]):
            raise ValueError("dependency pseudo-version does not match its exact commit")
    return sha


def type_reference_coverage(declarations: list[dict], module_path: str) -> dict:
    declared = {entry["id"] for entry in declarations}
    references = set()
    opaque = Counter()
    def visit(shape):
        if shape["kind"] == "named":
            package = shape["package"].removeprefix(module_path + "/")
            references.add(package + "#" + shape["name"])
        if shape["kind"] in {"opaque", "dynamic"}:
            opaque[shape["kind"]] += 1
        for key in ("element", "key", "target"):
            if key in shape:
                visit(shape[key])
        for argument in shape.get("arguments", []):
            visit(argument)
    for entry in declarations:
        visit(entry["shape"])
        for field in entry["fields"]:
            visit(field["shape"])
    return {"namedReferences": len(references), "resolvedNamedReferences": len(references & declared),
            "unresolvedNamedReferences": sorted(references - declared), "opaqueShapes": dict(sorted(opaque.items()))}


def source_inventory(mirror: Path, sha: str, prefixes: list[str], extractor: Path) -> dict:
    names = run(["git", "-C", str(mirror), "ls-tree", "-r", "--name-only", sha]).decode().splitlines()
    names = [name for name in names if name.endswith(".go") and not name.endswith("_test.go") and any(name.startswith(prefix) for prefix in prefixes)]
    requests = b"".join(f"{sha}:{name}\n".encode() for name in names)
    response = run(["git", "-C", str(mirror), "cat-file", "--batch"], data=requests)
    files = []
    offset = 0
    for name in names:
        end = response.index(b"\n", offset)
        header = response[offset:end].split()
        if len(header) != 3 or header[1] != b"blob":
            raise ValueError("source is not a Git blob")
        size = int(header[2])
        offset = end + 1
        content = response[offset:offset + size].decode()
        files.append({"path": name, "content": content})
        offset += size + 1
    if offset != len(response):
        raise ValueError("unexpected source stream content")
    result = json.loads(run([str(extractor)], data=canonical({"modulePath": source_module(mirror, sha), "files": files})))
    result["fileCount"] = len(files)
    return result


def source_shape(entry: dict) -> dict:
    """Compare structure independently of file location and explanatory comments."""
    return {key: ([{k: v for k, v in field.items() if k != "source"} for field in value] if key == "fields" else value)
            for key, value in entry.items() if key not in {"source", "id", "package", "name"}}


def structural_changes(previous: list[dict], current: list[dict]) -> dict:
    index = lambda entries: {(entry["id"], entry["buildConstraint"]): entry for entry in entries}
    before, after = index(previous), index(current)
    if len(before) != len(previous) or len(after) != len(current):
        raise ValueError("configuration declaration has ambiguous build conditions")
    removed, added = set(before) - set(after), set(after) - set(before)
    relocated = []
    for old in sorted(removed.copy()):
        matches = [new for new in added if source_shape(before[old]) == source_shape(after[new])]
        reverse = [other for other in removed if source_shape(before[other]) == source_shape(before[old])]
        if len(matches) == 1 and len(reverse) == 1:
            new = matches[0]
            relocated.append({"from": old[0], "to": new[0], "source": after[new]["source"]})
            removed.remove(old)
            added.remove(new)
    changed = []
    for key in sorted(set(before) & set(after)):
        a, b = before[key], after[key]
        if source_shape(a) == source_shape(b):
            continue
        a_fields = {field["name"]: field for field in a["fields"]}
        b_fields = {field["name"]: field for field in b["fields"]}
        changed.append({"id": key[0], "buildConstraint": key[1], "source": b["source"],
                        "addedFields": sorted(set(b_fields) - set(a_fields)),
                        "removedFields": sorted(set(a_fields) - set(b_fields)),
                        "changedFields": sorted(name for name in set(a_fields) & set(b_fields)
                                                if {k: v for k, v in a_fields[name].items() if k != "source"} != {k: v for k, v in b_fields[name].items() if k != "source"})})
    return {"added": [{"id": key[0], "buildConstraint": key[1], "source": after[key]["source"]} for key in sorted(added)],
            "removed": [{"id": key[0], "buildConstraint": key[1], "source": before[key]["source"]} for key in sorted(removed)],
            "changed": changed, "relocated": relocated}


def behavior_changes(previous: list[dict], current: list[dict]) -> list[dict]:
    # A changed function requires review; its hash is not a capability assertion.
    index = lambda entries: {(entry["id"], entry["buildConstraint"], entry["source"]["path"]): entry for entry in entries}
    before, after = index(previous), index(current)
    if len(before) != len(previous) or len(after) != len(current):
        raise ValueError("source behavior has ambiguous build conditions")
    result = []
    for key in sorted(set(before) | set(after)):
        a, b = before.get(key), after.get(key)
        if a and b and a["bodyHash"] == b["bodyHash"] and a.get("imports", {}) == b.get("imports", {}):
            continue
        result.append({"id": key[0], "buildConstraint": key[1], "source": (b or a)["source"],
                       "beforeHash": a["bodyHash"] if a else None, "afterHash": b["bodyHash"] if b else None,
                       "beforeImports": a.get("imports", {}) if a else None,
                       "afterImports": b.get("imports", {}) if b else None})
    return result


def decoder_changes(previous: list[dict], current: list[dict]) -> list[dict]:
    def index(entries):
        indexed = {(tuple(entry["path"]), entry["discriminator"], entry["value"], entry["buildConstraint"]): entry for entry in entries}
        if len(indexed) != len(entries):
            raise ValueError("decoder dispatch has ambiguous build conditions")
        return indexed
    before, after = index(previous), index(current)
    changes = []
    for key in sorted(set(before) | set(after)):
        a, b = before.get(key), after.get(key)
        if a == b:
            continue
        summarize = lambda entry: ({**{field: entry[field] for field in (
            "declaration", "optionsPath", "encoding", "discriminatorComparison", "objectLayout",
            "constructor", "constructorStatus")}, "evidenceHash": digest(entry["evidence"])} if entry else None)
        changes.append({"path": list(key[0]), "discriminator": key[1], "value": key[2], "buildConstraint": key[3], "before": summarize(a), "after": summarize(b)})
    return changes


def reviewed_rules(program: str, inventory: dict, rules: list[dict]) -> list[dict]:
    result = []
    for rule in rules:
        if rule["program"] != program:
            continue
        declarations = [entry for entry in inventory["declarations"] if entry["id"] == rule["declaration"]]
        if len(declarations) != 1:
            raise ValueError(f"semantic rule evidence is missing or ambiguous: {rule['id']}")
        declaration = declarations[0]
        fields = [field for field in declaration["fields"] if field["name"] == rule["field"]]
        if not fields:
            continue
        if len(fields) != 1:
            raise ValueError(f"semantic rule field requires source review: {rule['id']}")
        field_shape = {key: fields[0][key] for key in ("type", "embedded", "tags", "annotations")}
        if field_shape not in rule["fieldShapes"]:
            raise ValueError(f"semantic rule requires source review: {rule['id']}")
        if "evidenceVariants" in rule:
            variants = [variant for variant in rule["evidenceVariants"] if all(
                any(fn["id"] == recipe["function"] and fn["source"]["path"] == recipe["path"]
                    and fn["bodyHash"] in recipe["bodyHashes"] for fn in inventory["functions"])
                for recipe in variant["evidence"])]
            if not variants:
                raise ValueError(f"semantic rule parser requires source review: {rule['id']}")
            if any(variant["constraint"] != variants[0]["constraint"] for variant in variants):
                raise ValueError(f"semantic rule parser is ambiguous: {rule['id']}")
            rule = {**rule, **variants[0]}
        evidence = []
        for recipe in rule["evidence"]:
            functions = [fn for fn in inventory["functions"] if fn["id"] == recipe["function"]
                         and fn["source"]["path"] == recipe["path"]]
            if len(functions) != 1:
                raise ValueError(f"semantic rule behavior is missing or ambiguous: {rule['id']}")
            fn = functions[0]
            if (fn["bodyHash"] not in recipe["bodyHashes"] or fn["buildConstraint"] not in recipe["buildConstraints"]
                    or any(fn["imports"].get(name) != module for name, module in recipe.get("imports", {}).items())):
                raise ValueError(f"semantic rule behavior requires source review: {rule['id']}")
            evidence.append({"source": fn["source"], "bodyHash": fn["bodyHash"]})
        if not evidence:
            raise ValueError(f"semantic rule lacks reviewed behavior: {rule['id']}")
        result.append({**{key: value for key, value in rule.items() if key not in {"program", "evidence", "evidenceVariants"}},
                       "fieldHash": digest(field_shape),
                       "evidence": evidence})
        result[-1].pop("fieldShapes")
    return result


def apply_reviewed_layouts(program: str, inventory: dict, recipes: list[dict]) -> None:
    for recipe in recipes:
        if recipe["program"] != program:
            continue
        common_evidence = []
        conditions = set()
        def require_behavior(rule):
            candidates = [fn for fn in inventory["functions"] if fn["id"] == rule["function"]]
            selected = []
            unsupported = 0
            for fn in candidates:
                if fn["bodyHash"] in rule.get("unsupportedBodyHashes", []) and fn["buildConstraint"] == "(without_contextjson)":
                    unsupported += 1
                    continue
                if (fn["bodyHash"] not in rule["bodyHashes"]
                        or fn["buildConstraint"] not in rule.get("buildConstraints", [""])
                        or any(fn["imports"].get(alias) != target for alias, target in rule.get("imports", {}).items())):
                    raise ValueError(f"flat decoder behavior requires source review: {rule['function']}")
                selected.append(fn)
            if len(selected) != 1:
                raise ValueError(f"flat decoder behavior is missing or ambiguous: {rule['function']}")
            fn = selected[0]
            if unsupported > 1 or (unsupported and fn["buildConstraint"] != "(!without_contextjson)"):
                raise ValueError("flat decoder build branches overlap")
            if fn["buildConstraint"]:
                conditions.add(fn["buildConstraint"])
            return {"source": deepcopy(fn["source"]), "bodyHash": fn["bodyHash"]}
        for rule in recipe["evidence"]:
            common_evidence.append(require_behavior(rule))
        for root in recipe["roots"]:
            evidence = [*common_evidence, require_behavior(root["evidence"])]
            envelopes = [entry for entry in inventory["declarations"] if entry["id"] == root["envelope"]]
            if len(envelopes) != 1 or envelopes[0]["buildConstraint"]:
                raise ValueError("flat decoder envelope is missing or conditional")
            fields = [{key: field[key] for key in ("name", "type", "embedded", "tags")} for field in envelopes[0]["fields"]]
            if fields != recipe["envelopeFields"]:
                raise ValueError("flat decoder envelope fields require source review")
            selected = [binding for binding in inventory["decoderBindings"] if binding["path"] == [root["collection"], "*"]]
            if not selected:
                raise ValueError("flat decoder registry is missing")
            for binding in selected:
                if binding["optionsPath"] or binding["encoding"] != "json" or binding["objectLayout"] is not None:
                    raise ValueError("flat decoder registry shape requires source review")
                binding["objectLayout"] = {"buildConstraint": " && ".join(sorted(conditions)),
                    "envelopeDeclarations": [root["envelope"]], "envelopeKeyComparison": "exact",
                    "sharedOptions": [], "keyComparison": "asciiCaseInsensitive", "rootFieldNames": True, "nestedEmbedded": True}
                binding["evidence"].extend(deepcopy(evidence))


def reviewed_value_decoders(program: str, inventory: dict, recipes: list[dict], layouts: list[dict]) -> list[dict]:
    result = []
    for recipe in recipes:
        if recipe["program"] != program:
            continue
        conditions, evidence = set(), []
        def unique_declaration(name):
            matches = [entry for entry in inventory["declarations"] if entry["id"] == name]
            return matches[0] if len(matches) == 1 else None
        declaration = unique_declaration(recipe["declaration"])
        object_declaration = unique_declaration(recipe["objectDeclaration"])
        if (not declaration or not object_declaration or declaration["buildConstraint"]
                or object_declaration["buildConstraint"] or object_declaration["methods"]
                or "UnmarshalJSON" not in declaration["methods"]
                or object_declaration["shape"] != {"kind": "structure"}
                or [{key: field[key] for key in ("name", "type", "embedded", "tags")}
                    for field in object_declaration["fields"]] != recipe["objectFields"]):
            raise ValueError("custom decoder declarations require source review")
        shared = {entry["function"]: entry for layout in layouts if layout["program"] == program for entry in layout["evidence"]}
        rules = [*recipe["evidence"], *[shared[name] for name in recipe["layoutEvidence"]]]
        for rule in rules:
            candidates = [fn for fn in inventory["functions"] if fn["id"] == rule["function"]]
            supported = []
            branches = rule.get("branches", [{**rule, "supported": True}])
            seen = set()
            for fn in candidates:
                matching = [branch for branch in branches
                            if fn["bodyHash"] in branch["bodyHashes"]
                            and fn["buildConstraint"] in branch.get("buildConstraints", [""])
                            and all(fn["imports"].get(alias) == target for alias, target in branch.get("imports", {}).items())]
                if len(matching) != 1 or fn["buildConstraint"] in seen:
                    raise ValueError(f"custom decoder behavior requires source review: {rule['function']}")
                seen.add(fn["buildConstraint"])
                evidence.append({"source": deepcopy(fn["source"]), "bodyHash": fn["bodyHash"]})
                if matching[0]["supported"]:
                    supported.append(fn)
            if len(supported) != 1:
                raise ValueError(f"custom decoder behavior is missing or ambiguous: {rule['function']}")
            if supported[0]["buildConstraint"]:
                conditions.add(supported[0]["buildConstraint"])
        result.append({**{key: recipe[key] for key in ("declaration", "objectDeclaration", "encoding", "scalarForms", "keyComparison")},
                       "buildConstraint": " && ".join(sorted(conditions)), "evidence": evidence})
    return result


def build(cache: Path) -> tuple[dict, dict, dict]:
    producer = producer_hash()
    cache.mkdir(parents=True, exist_ok=True)
    extractor = cache / "core-knowledge-extractor"
    run(["go", "build", "-trimpath", "-o", str(extractor), "."], cwd=ROOT / "scripts/core-knowledge-extractor")
    programs = []
    coverage = []
    changes = []
    reviewed = json.loads(RULES.read_text())
    rules = reviewed["rules"]
    dependencies_cache = {}
    for descriptor in PROGRAMS:
        latest, families, selected = discover(descriptor)
        print(f"Inspecting {descriptor['program']}: {len(selected)} stable releases", file=sys.stderr, flush=True)
        mirror = cache / (descriptor["program"] + ".git")
        if not mirror.exists():
            run(["git", "init", "--bare", str(mirror)])
        refs = [f"refs/tags/{release['tag_name']}:refs/tags/{release['tag_name']}" for release in selected]
        run(["git", "-C", str(mirror), "fetch", "--depth=1", f"https://github.com/{descriptor['repository']}.git", *refs], timeout=600)
        releases = []
        variants: dict[str, dict] = {}
        rule_variants: dict[str, dict] = {}
        binding_variants: dict[str, dict] = {}
        value_decoder_variants: dict[str, dict] = {}
        build_report_variants: dict[str, dict] = {}
        release_coverage = []
        release_changes = []
        previous_inventory = None
        previous_release = None
        for release in selected:
            tag = release["tag_name"]
            sha = run(["git", "-C", str(mirror), "rev-parse", f"{tag}^{{commit}}"]).decode().strip()
            timestamp = run(["git", "-C", str(mirror), "show", "-s", "--format=%cI", sha]).decode().strip()
            if not SHA.fullmatch(sha):
                raise ValueError("release does not resolve to an exact commit")
            manifest = source_module_manifest(mirror, sha)
            module_path = manifest["Module"]["Path"]
            try:
                inventory = attribute_inventory(source_inventory(mirror, sha, descriptor["prefixes"], extractor), module_path, dependency=False)
                dependencies = []
                for dependency in DEPENDENCIES.get(descriptor["program"], []):
                    key = (dependency["modulePath"], dependency_version(manifest, dependency["modulePath"]))
                    if key not in dependencies_cache:
                        dependencies_cache[key] = dependency_inventory(cache, manifest, dependency, extractor)
                    identity, dependency_sources = deepcopy(dependencies_cache[key])
                    dependencies.append(identity)
                    for collection in ("declarations", "methods", "functions", "decoderBindings", "reportedBuildTags"):
                        inventory[collection].extend(dependency_sources[collection])
                    inventory["fileCount"] += dependency_sources["fileCount"]
            except (RuntimeError, ValueError) as error:
                raise RuntimeError(f"{descriptor['program']} {tag}: {error}") from error
            declarations = inventory["declarations"]
            ids = {entry["id"] for entry in declarations}
            if not set(descriptor["roots"]).issubset(ids):
                raise ValueError(f"configuration root is missing for {descriptor['program']} {tag}")
            for entry in declarations:
                # Immutable file/symbol references survive unrelated source line movement.
                entry["source"].pop("line")
                for field in entry["fields"]:
                    field["source"].pop("line")
                key = digest(entry)
                variants.setdefault(key, {"declaration": entry, "releases": []})["releases"].append(tag)
            for fn in inventory["functions"]:
                fn["source"].pop("line")
            apply_reviewed_layouts(descriptor["program"], inventory, reviewed["flatDecoders"])
            value_decoders = reviewed_value_decoders(descriptor["program"], inventory, reviewed["valueDecoders"], reviewed["flatDecoders"])
            inventory["valueDecoders"] = value_decoders
            for decoder in value_decoders:
                value_decoder_variants.setdefault(digest(decoder), {"decoder": decoder, "releases": []})["releases"].append(tag)
            bindings = inventory["decoderBindings"]
            for binding in bindings:
                if binding["declaration"] not in ids:
                    raise ValueError(f"decoder declaration is missing: {descriptor['program']} {tag} {binding['declaration']}")
                layout = binding["objectLayout"]
                if layout:
                    references = [*layout["envelopeDeclarations"], *(entry["declaration"] for entry in layout["sharedOptions"])]
                    if any(reference not in ids for reference in references):
                        raise ValueError(f"decoder object declaration is missing: {descriptor['program']} {tag}")
                for evidence in binding["evidence"]:
                    evidence["source"].pop("line", None)
                binding_variants.setdefault(digest(binding), {"binding": binding, "releases": []})["releases"].append(tag)
            build_reports = inventory["reportedBuildTags"]
            for report in build_reports:
                for evidence in report["evidence"]:
                    evidence["source"].pop("line")
                build_report_variants.setdefault(digest(report), {"report": report, "releases": []})["releases"].append(tag)
            semantic_rules = reviewed_rules(descriptor["program"], inventory, rules)
            for rule in semantic_rules:
                rule_variants.setdefault(digest(rule), {"rule": rule, "releases": []})["releases"].append(tag)
            current_release = {"tag": tag, "version": tag.removeprefix("v"), "commitSha": sha, "modulePath": module_path, "dependencies": dependencies, "publishedAt": release["published_at"], "sourceTimestamp": timestamp, "sourceUrl": f"https://github.com/{descriptor['repository']}/tree/{sha}"}
            releases.append(current_release)
            if previous_inventory:
                release_changes.append({"from": previous_release["tag"], "to": tag,
                                        "fromCommit": previous_release["commitSha"], "toCommit": sha,
                                        "compareUrl": f"https://github.com/{descriptor['repository']}/compare/{previous_release['commitSha']}...{sha}",
                                        "dependenciesChanged": previous_release["dependencies"] != dependencies,
                                        "structure": structural_changes(previous_inventory["declarations"], declarations),
                                        "decoderBindings": decoder_changes(previous_inventory["decoderBindings"], bindings),
                                        "buildTagReportsChanged": previous_inventory["reportedBuildTags"] != build_reports,
                                        "valueDecodersChanged": previous_inventory["valueDecoders"] != value_decoders,
                                        "behaviorReview": behavior_changes(previous_inventory["functions"], inventory["functions"])})
            previous_inventory, previous_release = inventory, current_release
            encodings = Counter(encoding for entry in declarations for field in entry["fields"] for encoding, value in field["tags"].items() if value.split(",")[0] not in ("", "-"))
            release_coverage.append({"tag": tag, "typeReferences": type_reference_coverage(declarations, module_path), "sourceFiles": inventory["fileCount"], "declarations": len(declarations), "taggedFields": dict(sorted(encodings.items())), "customDecoders": sum(any(name.startswith("Unmarshal") for name in entry["methods"]) for entry in declarations), "reviewedValueDecoders": len(value_decoders), "schemaAnnotatedFields": sum(bool(field["annotations"]) for entry in declarations for field in entry["fields"]), "reviewedSemanticRules": len(semantic_rules), "decoderBindings": len(bindings), "decodedObjectLayouts": sum(binding["objectLayout"] is not None for binding in bindings), "reportedBuildTags": len(build_reports), "rejectedConstructors": sum(binding["constructorStatus"] == "rejected" for binding in bindings)})
        programs.append({"program": descriptor["program"], "repository": descriptor["repository"], "familyPolicy": descriptor["familyPolicy"], "latestStable": latest["tag_name"], "families": [f"{major}.{minor}" for major, minor in families], "configurationRoots": descriptor["roots"], "outboundCollection": descriptor["outboundCollection"], "shareProtocols": descriptor["shareProtocols"], "releases": releases, "declarations": [variants[key] for key in sorted(variants)], "semanticRules": [rule_variants[key] for key in sorted(rule_variants)], "decoderBindings": [binding_variants[key] for key in sorted(binding_variants)], "valueDecoders": [value_decoder_variants[key] for key in sorted(value_decoder_variants)], "reportedBuildTags": [build_report_variants[key] for key in sorted(build_report_variants)]})
        coverage.append({"program": descriptor["program"], "releases": release_coverage})
        changes.append({"program": descriptor["program"], "releases": release_changes})
    if producer_hash() != producer:
        raise ValueError("knowledge producer changed during source extraction")
    body = {"producerHash": producer, "reportsHash": digest({"coverage": coverage, "changes": changes}), "programs": programs}
    artifact = {"contentHash": digest(body), **body}
    report = {"knowledgeHash": artifact["contentHash"], "programs": coverage}
    return artifact, report, {"knowledgeHash": artifact["contentHash"], "programs": changes}


def validate(artifact: dict) -> None:
    if set(artifact) != {"contentHash", "producerHash", "reportsHash", "programs"} or artifact["contentHash"] != digest({key: value for key, value in artifact.items() if key != "contentHash"}):
        raise ValueError("knowledge content hash mismatch")
    if artifact["producerHash"] != producer_hash():
        raise ValueError("knowledge must be regenerated with the current extractor and reviewed rules")
    descriptors = {entry["program"]: entry for entry in PROGRAMS}
    if len(artifact["programs"]) != len(descriptors) or {entry["program"] for entry in artifact["programs"]} != set(descriptors):
        raise ValueError("knowledge program registry mismatch")
    for program in artifact["programs"]:
        descriptor = descriptors[program["program"]]
        if program["repository"] != descriptor["repository"] or program["familyPolicy"] != descriptor["familyPolicy"] or program["configurationRoots"] != descriptor["roots"] or program["outboundCollection"] != descriptor["outboundCollection"] or program["shareProtocols"] != descriptor["shareProtocols"]:
            raise ValueError("program knowledge descriptor mismatch")
        tags = set()
        families = set()
        source_modules = {}
        for release in program["releases"]:
            version = stable_version(release["tag"])
            if not version or release["version"] != release["tag"].removeprefix("v") or release["tag"] in tags or not SHA.fullmatch(release["commitSha"]) or not release["modulePath"]:
                raise ValueError("invalid stable release identity")
            if descriptor["familyPolicy"] == "releaseMonth" and not 1 <= version[1] <= 12:
                raise ValueError("release family must name a calendar month")
            if release["sourceUrl"] != f"https://github.com/{program['repository']}/tree/{release['commitSha']}":
                raise ValueError("source URL must bind the exact release commit")
            modules = {release["modulePath"]}
            expected_dependencies = {entry["modulePath"]: entry for entry in DEPENDENCIES.get(program["program"], [])}
            for dependency in release["dependencies"]:
                reviewed = expected_dependencies.get(dependency["modulePath"])
                if (not reviewed or dependency["modulePath"] in modules
                        or dependency["repository"] != reviewed["repository"]
                        or not SHA.fullmatch(dependency["commitSha"])
                        or dependency["sourceUrl"] != f"https://github.com/{dependency['repository']}/tree/{dependency['commitSha']}"):
                    raise ValueError("dependency source must bind a reviewed module and exact commit")
                dependency_version({"Require": [{"Path": dependency["modulePath"], "Version": dependency["version"]}]}, dependency["modulePath"])
                modules.add(dependency["modulePath"])
            if modules - {release["modulePath"]} != set(expected_dependencies):
                raise ValueError("release lacks reviewed dependency sources")
            source_modules[release["tag"]] = modules
            tags.add(release["tag"])
            families.add(f"{version[0]}.{version[1]}")
        if len(program["families"]) != 2 or families != set(program["families"]) or program["latestStable"] not in tags:
            raise ValueError("knowledge must cover exactly two stable families")
        def validate_sources(value, releases):
            if isinstance(value, list):
                for entry in value:
                    validate_sources(entry, releases)
            elif isinstance(value, dict):
                if "source" in value:
                    source = value["source"]
                    if (not source.get("symbol") or not source.get("path")
                            or "\\" in source["path"]
                            or any(part in {"", ".", ".."} for part in source["path"].split("/"))
                            or any(source.get("modulePath") not in source_modules.get(tag, set()) for tag in releases)):
                        raise ValueError("source reference lacks exact module identity")
                for entry in value.values():
                    validate_sources(entry, releases)
        for collection in ("declarations", "semanticRules", "decoderBindings", "valueDecoders", "reportedBuildTags"):
            for variant in program[collection]:
                validate_sources(variant, variant["releases"])
        for variant in program["declarations"]:
            if not variant["releases"] or not set(variant["releases"]).issubset(tags) or len(set(variant["releases"])) != len(variant["releases"]):
                raise ValueError("declaration references an invalid release")
        for variant in program["semanticRules"]:
            if not variant["releases"] or not set(variant["releases"]).issubset(tags):
                raise ValueError("semantic rule references an invalid release")
        seen_reports = set()
        seen_decoders = set()
        for variant in program["valueDecoders"]:
            decoder = variant["decoder"]
            if not variant["releases"] or not set(variant["releases"]).issubset(tags) or not decoder["evidence"]:
                raise ValueError("custom decoder lacks release or source evidence")
            for tag in variant["releases"]:
                key = (tag, decoder["declaration"])
                if key in seen_decoders:
                    raise ValueError("custom decoder must be unique per stable release")
                seen_decoders.add(key)
                for name in (decoder["declaration"], decoder["objectDeclaration"]):
                    if not any(entry["declaration"]["id"] == name and tag in entry["releases"] for entry in program["declarations"]):
                        raise ValueError("custom decoder lacks exact source declarations")
        for variant in program["reportedBuildTags"]:
            report = variant["report"]
            if not variant["releases"] or not set(variant["releases"]).issubset(tags) or len(report["evidence"]) < 4:
                raise ValueError("build report lacks release or source evidence")
            for tag in variant["releases"]:
                key = (tag, report["tag"])
                if key in seen_reports:
                    raise ValueError("build report must be unique per stable release")
                seen_reports.add(key)
        for variant in program["decoderBindings"]:
            if not variant["releases"] or not set(variant["releases"]).issubset(tags) or not variant["binding"]["evidence"]:
                raise ValueError("decoder binding references an invalid release or lacks evidence")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rebuild", action="store_true")
    parser.add_argument("--write", action="store_true")
    parser.add_argument("--cache", type=Path)
    parser.add_argument("--check-upstream", action="store_true", help="compare the maintained stable window and exact tags with official upstreams without writing")
    args = parser.parse_args()
    try:
        if args.rebuild:
            if args.cache is None:
                raise ValueError("rebuild requires an isolated cache directory")
            artifact, report, changes = build(args.cache.resolve())
        else:
            if args.write:
                raise ValueError("write requires rebuild")
            artifact = json.loads(ARTIFACT.read_text())
            report = json.loads(REPORT.read_text())
            changes = json.loads(CHANGES.read_text())
        validate(artifact)
        if report["knowledgeHash"] != artifact["contentHash"] or changes["knowledgeHash"] != artifact["contentHash"]:
            raise ValueError("coverage report does not match knowledge")
        if artifact["reportsHash"] != digest({"coverage": report["programs"], "changes": changes["programs"]}):
            raise ValueError("source review reports do not match knowledge")
        if args.check_upstream:
            for descriptor in PROGRAMS:
                latest, families, selected = discover(descriptor)
                program = next(item for item in artifact["programs"] if item["program"] == descriptor["program"])
                if latest["tag_name"] != program["latestStable"] or {item["tag_name"] for item in selected} != {item["tag"] for item in program["releases"]}:
                    raise ValueError(f"stable window needs review: {descriptor['program']}")
                refs = {}
                for line in run(["git", "ls-remote", "--tags", f"https://github.com/{descriptor['repository']}.git"]).decode().splitlines():
                    sha, ref = line.split()
                    refs[ref] = sha
                for release in program["releases"]:
                    ref = f"refs/tags/{release['tag']}"
                    if refs.get(ref + "^{}", refs.get(ref)) != release["commitSha"]:
                        raise ValueError(f"stable source identity needs review: {descriptor['program']} {release['tag']}")
        if args.write:
            ARTIFACT.write_text(json.dumps(artifact, ensure_ascii=False, indent=2) + "\n")
            REPORT.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
            CHANGES.write_text(json.dumps(changes, ensure_ascii=False, indent=2) + "\n")
        print(f"Core knowledge verified: {artifact['contentHash']}")
        return 0
    except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired) as error:
        print(f"Core knowledge check failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
