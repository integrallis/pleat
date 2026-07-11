# Vector generator: compiles against the pinned reference ribbon headers.
REF := ../../transposed-filters/harness/fastfilter_cpp/src/ribbon
vecgen: vecgen.cc
	$(CXX) -O2 -std=c++17 -Wall -I$(REF) -o $@ $<
vectors: vecgen vecgen128
	./vecgen > ../tests/vectors/homog_w64_r7.json
	./vecgen128 > ../tests/vectors/std_w128_r7.json
	@echo "wrote tests/vectors/{homog_w64_r7,std_w128_r7}.json"

vecgen128: vecgen128.cc
	$(CXX) -O2 -std=c++17 -Wall -I$(REF) -o $@ $<
clean:
	rm -f vecgen vecgen128
.PHONY: vectors clean
