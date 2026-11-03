#!/usr/bin/env python3
"""Compile/link generated LO and validate exact output, exit status, and OOM diagnostics."""
import argparse
import os
from pathlib import Path
import platform
import re
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parent.parent
p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--target', choices=['wasm', 'native'], required=True)
p.add_argument('--compiler', type=Path, required=True, help='Liveoak compiler executable')
p.add_argument('--runtime-lib', type=Path, required=True, help='Archive built for the chosen target')
p.add_argument('--wasmrun', type=Path)
p.add_argument('--wasm-ld', default='wasm-ld')
p.add_argument('--cc', default='cc')
p.add_argument('--pattern', default='gc_meta_*.lo', help='Corpus filename glob (default: all generated cases)')
p.add_argument('--sweep-heaps', default='', help='Native-only extra heap sizes, comma-separated; success or diagnosed OOM recorded')
p.add_argument('--timeout', type=int, default=120)
a = p.parse_args()
if a.target == 'native' and (platform.system() != 'Linux' or platform.machine() not in ['x86_64','AMD64']):
    p.error('native execution requires Linux x86-64 (a suitable container is fine)')
if a.target == 'wasm' and not a.wasmrun: p.error('--wasmrun is required for WASM')
try:
    sweep = [str(int(h)) for h in a.sweep_heaps.split(',') if h]
    if any(int(h) <= 0 for h in sweep): raise ValueError()
except ValueError: p.error('--sweep-heaps requires positive integer byte sizes')
if sweep and a.target != 'native': p.error('heap sweeps are native-only')
compiler = str(a.compiler.resolve()); archive = str(a.runtime_lib.resolve())
failures = 0
with tempfile.TemporaryDirectory(prefix='lo-gc-runs-') as work:
    sources = sorted((ROOT/'tests/lo_programs').glob(a.pattern))
    if not sources: p.error('no programs match --pattern')
    for src in sources:
        try:
            text = src.read_text(); name = src.stem
            expected_code = int(re.search(r'expected exit code: (\d+)', text)[1]) if 'expected exit code:' in text else 0
            expected = (ROOT/'tests/expected'/f'{name}.expected.out').read_bytes()
            subprocess.run([compiler, 'check', str(src)], check=True, timeout=a.timeout, capture_output=True)
            obj=Path(work)/(name+'.o'); exe=Path(work)/(name+('.wasm' if a.target=='wasm' else '.native'))
            if a.target == 'wasm':
                subprocess.run([compiler,'compile',str(src),'-o',str(obj)],check=True,timeout=a.timeout,capture_output=True)
                subprocess.run([a.wasm_ld,'--no-entry','--export=lo_entry','--export-memory','--allow-undefined',str(obj),archive,'-o',str(exe)],check=True,timeout=a.timeout,capture_output=True)
                command=[str(a.wasmrun.resolve()),str(exe)]; heaps=[None]
            else:
                asm=Path(work)/(name+'.s')
                with asm.open('w') as out:
                    subprocess.run([compiler,'native-asm',str(src)],stdout=out,stderr=subprocess.PIPE,check=True,timeout=a.timeout)
                subprocess.run([a.cc,'-no-pie',str(asm),archive,'-lpthread','-ldl','-lm','-o',str(exe)],check=True,timeout=a.timeout,capture_output=True)
                command=[str(exe)]; heaps=[None,re.search(r'gc stress heap: (\d+)',text)[1]]
            trials = [(h, False) for h in heaps] + ([(h, True) for h in sweep] if expected_code == 0 else [])
            for heap, sweeping in trials:
                env=dict(os.environ);env.pop('LO_HEAP_SIZE',None)
                if heap is not None: env['LO_HEAP_SIZE']=heap
                r=subprocess.run(command,capture_output=True,timeout=a.timeout,env=env)
                ok=(r.returncode==expected_code and r.stdout==expected and
                    (expected_code!=137 or b'lo_alloc: out of memory' in r.stderr))
                if sweeping:
                    ok = ((r.returncode == 0 and r.stdout == expected) or
                          (r.returncode == 137 and b'lo_alloc: out of memory' in r.stderr and expected.startswith(r.stdout)))
                label = ('OOM' if r.returncode == 137 else 'SUCCESS') if sweeping and ok else ('PASS' if ok else 'FAIL')
                print(f'{name} {a.target}@{heap or "default"}{" sweep" if sweeping else ""}: {label} (exit {r.returncode})',flush=True)
                if not ok:
                    failures+=1
                    print('stdout prefix:',repr(r.stdout[:160]),'stderr:',r.stderr[:400].decode(errors='replace'))
        except (subprocess.SubprocessError,OSError) as e:
            failures+=1; print(f'{src.name}: FAIL: {e}',flush=True)
            if isinstance(e,subprocess.CalledProcessError): print(e.stderr.decode(errors='replace') if e.stderr else '')
print(f'Failures: {failures}')
raise SystemExit(bool(failures))
