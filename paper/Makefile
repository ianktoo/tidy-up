# Build the paper.
#
#   make            pdflatex, three passes (references, then cleveref)
#   make lua        lualatex, for full Unicode including the CJK sample
#   make xe         xelatex, same
#   make clean      remove build artifacts, keep the PDF
#   make distclean  remove everything generated

MAIN    := tidy-up
FLAGS   := -interaction=nonstopmode -file-line-error
ARTIFACTS := $(MAIN).aux $(MAIN).log $(MAIN).out $(MAIN).toc $(MAIN).lof \
             $(MAIN).lot $(MAIN).fls $(MAIN).fdb_latexmk $(MAIN).synctex.gz

.PHONY: all lua xe clean distclean check

all: $(MAIN).pdf

$(MAIN).pdf: $(MAIN).tex
	pdflatex $(FLAGS) $(MAIN).tex
	pdflatex $(FLAGS) $(MAIN).tex
	pdflatex $(FLAGS) $(MAIN).tex

lua: $(MAIN).tex
	lualatex $(FLAGS) $(MAIN).tex
	lualatex $(FLAGS) $(MAIN).tex

xe: $(MAIN).tex
	xelatex $(FLAGS) $(MAIN).tex
	xelatex $(FLAGS) $(MAIN).tex

# Fails loudly if the last build produced anything at all to complain about.
check:
	@grep -c '^! ' $(MAIN).log | grep -qx 0 || { echo "LaTeX errors"; exit 1; }
	@grep -ci 'Missing character' $(MAIN).log | grep -qx 0 || { echo "missing glyphs"; exit 1; }
	@echo "clean build"

clean:
	rm -f $(ARTIFACTS)

distclean: clean
	rm -f $(MAIN).pdf
