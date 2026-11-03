#!/usr/bin/env python3
"""Small semantic regression checks and byte-for-byte corpus reproducibility."""
from pathlib import Path
import subprocess
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
COMPILER = HERE / 'gc-metalang'

class Semantics(unittest.TestCase):
    def compile(self, body, functions='', args=(), ok=True):
        with tempfile.TemporaryDirectory(prefix='lo-gc-check-') as d:
            src = Path(d) / 'case.lo_gc'
            src.write_text('heap 65536\nexpect success\n' + functions + '\nfunc main 0\n' + body + '\nend\n')
            result = subprocess.run([str(COMPILER), str(src), *args], capture_output=True, text=True)
            if not ok:
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertFalse(src.with_suffix('.lo').exists())
                return result.stderr
            self.assertEqual(result.returncode, 0, result.stderr)
            return src.with_suffix('.lo').read_text(), src.with_suffix('.expected.out').read_text()

    def test_temporary_is_only_literal_and_newest_first(self):
        code, expected = self.compile('alloc 12 ordinary\nalloc 4 t0\ncall use', 'func use 2\nend\n')
        self.assertIn('("bbbb", v0);', code)
        self.assertNotIn('t0', code)
        self.assertNotIn('= ("b" * 4);', code)
        self.assertIn('("a" * 12)', code)
        self.assertEqual(expected, 'true\n' * 8)

    def test_prefix_is_exact(self):
        code, _ = self.compile('alloc 4 tiny\nalloc 4 temp\nalloc 4 t01\ncall use', 'func use 1\nend\n')
        self.assertIn('("a" * 4)', code)
        self.assertIn('("b" * 4)', code)
        self.assertIn('("cccc");', code)

    def test_aliases_do_not_fill_arity(self):
        self.assertIn('only 1 available', self.compile('alloc 4 a\nb = a\ncall use', 'func use 2\nend\n', ok=False))

    def test_null_excluded_empty_included(self):
        code, _ = self.compile('alloc 4 a\na = null\ne = ""\ncall use', 'func use 1\nend\n')
        self.assertIn('(v1);', code)
        self.assertNotIn('= null', code)

    def test_redefinition_and_null_temporaries_rejected(self):
        for body in ['alloc 4 t0\nt0 = ""', 't0 = null', 'a = null\nt0 = a']:
            with self.subTest(body=body): self.compile(body, ok=False)

    def test_limit_never_falls_back(self):
        self.assertIn('limit is 256', self.compile('alloc 257 t0', ok=False))
        code, _ = self.compile('alloc 257 t0\ncall use', 'func use 1\nend\n', args=('--literal-limit', '257'))
        self.assertIn('"'+'a'*257+'"', code)
        self.assertNotIn('String v', code)

    def test_parameter_forwarding_stays_variable(self):
        code, _ = self.compile('alloc 4 t0\ncall outer', 'func use 1\nend\nfunc outer 1\ncall use\nend\n')
        self.assertIn('(p0);', code)

    def test_snapshot_and_representative_order(self):
        code, _ = self.compile('alloc 4 a\nt0 = a\nalloc 8 a\ncall use', 'func use 2\nend\n')
        self.assertIn('(v0, "aaaa");', code)

    def test_invalid_programs(self):
        for body,functions in [('a = missing',''),('alloc -1 a',''),('call missing',''),
            ('call f','func f 0\ncall f\nend\n'),('alloc 1 arg9',''),
            ('alloc 1 a','func unused 0\nend\n')]:
            with self.subTest(body=body): self.compile(body, functions, ok=False)

    def test_distinct_equal_contents_and_repeated_calls(self):
        # Repeated invocation parameters must not be incorrectly reused across content contexts.
        code, expected = self.compile('alloc 1 a\ncall use\nalloc 2 a\ncall use', 'func use 1\nend\n')
        self.assertIn('(p0 = ("a" * 1))', code)
        self.assertIn('(p0 = ("b" * 2))', code)
        self.assertTrue(expected)

    def test_corpus_is_reproducible(self):
        for src in sorted((HERE.parent/'tests/gc_metalang').glob('*.lo_gc')):
            with self.subTest(src=src.name), tempfile.TemporaryDirectory(prefix='lo-gc-repro-') as d:
                lo=Path(d)/'out.lo'; expected=Path(d)/'out.expected.out'
                subprocess.run([str(COMPILER),str(src),'-o',str(lo),'--expected',str(expected)], check=True)
                self.assertEqual(lo.read_bytes(), (HERE.parent/'tests/lo_programs'/f'{src.stem}.lo').read_bytes())
                self.assertEqual(expected.read_bytes(), (HERE.parent/'tests/expected'/f'{src.stem}.expected.out').read_bytes())

if __name__ == '__main__': unittest.main()
