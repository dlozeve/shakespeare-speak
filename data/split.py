#!/usr/bin/env python3
"""Split the Project Gutenberg Complete Works of Shakespeare into one file per work.

Boundary detection
------------------
Every play in this edition is introduced by a ``Contents`` heading, and the
work's title is the nearest non-blank line *above* that heading.  Using the
heading rather than the title text matters: the table of contents lists
"KING RICHARD THE SECOND" while the body heading reads "THE LIFE AND DEATH OF
KING RICHARD THE SECOND", so matching on the TOC string alone would start that
work 42 lines too late and leave its front matter inside the previous file.

The Sonnets and the five poems have no ``Contents`` heading, so they are
located by an exact flush-left match on their title.
"""

import re
import unicodedata
from pathlib import Path

SRC = Path("complete.txt")
OUT = Path("works")

CONTENTS_HEADER = "Contents"
END_MARKER = "*** END OF THE PROJECT GUTENBERG EBOOK"


def slugify(title: str) -> str:
    text = unicodedata.normalize("NFKD", title)
    text = text.encode("ascii", "ignore").decode("ascii").lower()
    text = re.sub(r"[^a-z0-9]+", "-", text)
    return text.strip("-")


def main() -> None:
    lines = SRC.read_text(encoding="utf-8").splitlines()

    # --- The master table of contents -------------------------------------
    master = next(
        i for i, line in enumerate(lines) if line.strip() == CONTENTS_HEADER
    )
    master_end = next(
        i
        for i in range(master + 1, len(lines))
        if lines[i].strip() == ""
        and lines[i + 1].strip() == ""
        and lines[i + 2].strip() == ""
    )
    titles = [
        lines[i].strip() for i in range(master + 1, master_end) if lines[i].strip()
    ]

    body_end = next(i for i, line in enumerate(lines) if line.startswith(END_MARKER))

    # --- One boundary per play, anchored on its "Contents" heading --------
    anchored = []
    for i in range(master_end, body_end):
        if lines[i].strip() != CONTENTS_HEADER:
            continue
        j = i - 1
        while j > master_end and not lines[j].strip():
            j -= 1
        anchored.append(j)

    # The TOC lists THE SONNETS, then the 38 plays, then the five poems, so the
    # anchored boundaries line up with titles[1:1 + len(anchored)].
    if len(anchored) != len(titles) - 6:
        raise SystemExit(f"expected {len(titles) - 6} plays, found {len(anchored)}")

    taken = set(anchored)
    starts: list[int] = []
    for i, title in enumerate(titles):
        if 1 <= i <= len(anchored):
            starts.append(anchored[i - 1])
        else:
            # The Sonnets and the poems have no Contents heading.
            starts.append(
                next(
                    j
                    for j in range(master_end, body_end)
                    if lines[j] == title and j not in taken
                )
            )

    if starts != sorted(starts):
        raise SystemExit("boundaries are not in document order")

    # --- Write one file per work ------------------------------------------
    OUT.mkdir(exist_ok=True)
    bounds = starts + [body_end]
    for n, title in enumerate(titles):
        chunk = lines[bounds[n] : bounds[n + 1]]
        while chunk and not chunk[-1].strip():
            chunk.pop()
        text = "\n".join(chunk).rstrip("\n") + "\n"

        path = OUT / f"{slugify(title)}.txt"
        path.write_text(text, encoding="utf-8")
        print(f"{path.name:55s} L{bounds[n] + 1:<7d} {len(chunk):6d} lines  {title}")

    print(f"\n{len(titles)} works written to {OUT}/")


if __name__ == "__main__":
    main()
