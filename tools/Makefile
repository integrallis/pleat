# Vector generator: compiles against the pinned reference ribbon headers.
REF := ../../transposed-filters/harness/fastfilter_cpp/src/ribbon
vecgen: vecgen.cc
	$(CXX) -O2 -std=c++17 -Wall -I$(REF) -o $@ $<
vectors: vecgen
	./vecgen > ../tests/vectors/homog_w64_r7.json
	@echo "wrote tests/vectors/homog_w64_r7.json"
clean:
	rm -f vecgen
.PHONY: vectors clean
