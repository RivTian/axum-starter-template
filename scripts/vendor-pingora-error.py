#!/usr/bin/env python3
"""Vendors pingora-error into the template's util crate, or proves the vendored files.

usage:
  scripts/vendor-pingora-error.py [--upstream DIR]   write the vendored files
  scripts/vendor-pingora-error.py --check-patches    prove them (offline)

The upstream files are pingora-error/src/lib.rs and immut_str.rs at a pinned commit of
github.com/cloudflare/pingora; --upstream points at a local copy of that directory, otherwise
they are downloaded. Their sha256 must match the pinned values.

The vendored files are upstream plus exactly the PATCHES below: --check-patches undoes them
and requires the pinned upstream bytes. The upstream unit tests and doctests run with the
generated project's own tests.
"""
import hashlib
import os
import sys
import urllib.request

COMMIT = "4487f7b2ab50f159e4a2cf4f6a6b813f61bb6e19"
SHA256 = {
    "lib.rs": "6cf3982bf68a2ad1562786e8d3ca218858e52378099b737d8b134e3f5ebd93d3",
    "immut_str.rs": "b23c396f6e68d26be03b43de0c715a11dd3a85462ae099d0f3c9e37b69c97a6a",
}
URL = "https://raw.githubusercontent.com/cloudflare/pingora/{commit}/pingora-error/src/{name}"

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ERROR_DIR = os.path.join(ROOT, "template", "crates", "{{project-name}}-util", "src", "error")
TARGETS = {
    "lib.rs": os.path.join(ERROR_DIR, "pingora.rs"),
    "immut_str.rs": os.path.join(ERROR_DIR, "pingora", "immut_str.rs"),
}

HEADER_END = "// limitations under the License.\n"
NOTICE_LIB = HEADER_END + """//
// Modified by the project authors: copied from pingora-error 0.9.0
// (github.com/cloudflare/pingora, commit 4487f7b) and changed as follows: lint
// expectations added below and on `RetryType::retry`; the variant `ErrorType::Kind`
// added for the error kinds this project defines; doctest imports point at this
// module. See THIRD_PARTY_NOTICES.md for the license text.
"""
NOTICE_IMMUT_STR = HEADER_END + """//
// Modified by the project authors: copied from pingora-error 0.9.0
// (github.com/cloudflare/pingora, commit 4487f7b) without changes to the code.
"""

# (old, new, count): each `old` must occur exactly `count` times in upstream, and `new` not
# at all, so that the patch can be undone exactly.
PATCHES = {
    "lib.rs": [
        (HEADER_END, NOTICE_LIB, 1),
        ("#![warn(clippy::all)]\n",
         "#![warn(clippy::all)]\n"
         "#![expect(missing_docs, clippy::pedantic, clippy::uninlined_format_args)]\n"
         "#![cfg_attr(test, expect(clippy::unwrap_used))]\n", 1),
        ("use std::result::Result as StdResult;\n",
         "use std::result::Result as StdResult;\n\nuse super::ErrorKind;\n", 1),
        ("    CustomCode(&'static str, u16),\n}\n",
         "    CustomCode(&'static str, u16),\n"
         "    /// An error kind defined by this project (added by the project authors).\n"
         "    Kind(&'static ErrorKind),\n}\n", 1),
        ("            ErrorType::CustomCode(s, _) => s,\n",
         "            ErrorType::CustomCode(s, _) => s,\n"
         "            ErrorType::Kind(kind) => kind.name(),\n", 1),
        ("    pub fn retry(&self) -> bool {\n        match self {",
         "    #[expect(clippy::panic)]\n    pub fn retry(&self) -> bool {\n        match self {", 1),
        ("/// use pingora_error::", "/// use svc_util::error::", None),
    ],
    "immut_str.rs": [
        (HEADER_END, NOTICE_IMMUT_STR, 1),
    ],
}


def sha256(text):
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def apply(name, text):
    for old, new, count in PATCHES[name]:
        found = text.count(old)
        if (count is not None and found != count) or found == 0 or new in text:
            raise SystemExit(f"{name}: cannot apply patch at {old!r} (found {found} times)")
        text = text.replace(old, new)
    return text


def undo(name, text):
    for old, new, _ in reversed(PATCHES[name]):
        text = text.replace(new, old)
    return text


def read_upstream(directory):
    texts = {}
    for name, digest in SHA256.items():
        if directory:
            with open(os.path.join(directory, name), encoding="utf-8") as f:
                texts[name] = f.read()
        else:
            with urllib.request.urlopen(URL.format(commit=COMMIT, name=name), timeout=30) as r:
                texts[name] = r.read().decode("utf-8")
        if sha256(texts[name]) != digest:
            raise SystemExit(f"upstream {name}: sha256 {sha256(texts[name])} is not the pinned {digest}")
    return texts


def main():
    args = sys.argv[1:]
    if args == ["--check-patches"]:
        for name, path in TARGETS.items():
            with open(path, encoding="utf-8") as f:
                restored = undo(name, f.read())
            if sha256(restored) != SHA256[name]:
                raise SystemExit(f"{os.path.relpath(path, ROOT)} is not upstream {name} plus the listed patches")
        print(f"vendored pingora-error: upstream {COMMIT[:7]} plus exactly the listed patches")
        return
    directory = args[1] if args[:1] == ["--upstream"] else None
    for name, text in read_upstream(directory).items():
        os.makedirs(os.path.dirname(TARGETS[name]), exist_ok=True)
        with open(TARGETS[name], "w", encoding="utf-8") as f:
            f.write(apply(name, text))
        print(f"wrote {os.path.relpath(TARGETS[name], ROOT)}")


if __name__ == "__main__":
    main()
