OCAMLC ?= ocamlc
.PHONY: all corpus check clean
all: gc-metalang

gc-metalang: compiler.ml
	$(OCAMLC) -I +str -w +a-4-42-70 -o $@ str.cma compiler.ml

corpus: gc-metalang
	@set -eu; for src in ../tests/gc_metalang/*.lo_gc; do \
	  name=$$(basename "$$src" .lo_gc); \
	  ./gc-metalang "$$src" -o "../tests/lo_programs/$$name.lo" --expected "../tests/expected/$$name.expected.out"; \
	done

check: gc-metalang
	python3 check.py

clean:
	rm -f gc-metalang compiler.cmi compiler.cmo
