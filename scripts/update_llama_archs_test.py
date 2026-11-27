#!/usr/bin/env python3
"""Tests of scripts/update_llama_archs.py with a made-up llama-arch.cpp (no network)."""
import importlib.util
import pathlib
import subprocess
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("update_llama_archs", HERE / "update_llama_archs.py")
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)


def source(names):
    rows = "".join(f'    {{ LLM_ARCH_X{i}, "{n}" }},\n' for i, n in enumerate(names))
    return (
        "static const std::map<llm_arch, const char *> LLM_ARCH_NAMES = {\n"
        + rows
        + '    { LLM_ARCH_UNKNOWN, "(unknown)" },\n};\n\nstatic const other = { { LLM_ARCH_Z, "not-an-arch" } };\n'
    )


class Parse(unittest.TestCase):
    def test_reads_the_names_table_only(self):
        names = [f"arch{i}" for i in range(60)]
        self.assertEqual(mod.parse_names(source(names)), names)

    def test_a_changed_layout_is_an_error_not_an_empty_list(self):
        with self.assertRaises(SystemExit):
            mod.parse_names(source(["a", "b"]))

    def test_render_names_the_build_and_the_commit(self):
        text = mod.render("b1", "c" * 40, ["llama", "qwen3"])
        self.assertTrue(text.startswith("# general.architecture values known to llama.cpp b1 (" + "c" * 40 + ")."))
        self.assertEqual(mod.names_in(text), ["llama", "qwen3"])


class Checkout(unittest.TestCase):
    def test_a_folder_that_is_not_the_pinned_commit_is_refused(self):
        with tempfile.TemporaryDirectory() as d:
            subprocess.run(["git", "init", "-q", d], check=True)
            with self.assertRaises(SystemExit) as e:
                mod.read_source("0" * 40, pathlib.Path(d))
            self.assertIn("not a git checkout of the pinned commit", str(e.exception))

    def test_a_plain_folder_is_refused(self):
        with tempfile.TemporaryDirectory() as d:
            with self.assertRaises(SystemExit):
                mod.read_source("0" * 40, pathlib.Path(d))


if __name__ == "__main__":
    unittest.main()
