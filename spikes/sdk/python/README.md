# N4 Python authoring SDK

Open a saved project, find a named layer, edit its authored fields and validate the candidate through the same Rust reader the viewer uses. Python needs only the standard library and a built N4 CLI.

From the repository root:

```sh
just host-native
export PYTHONPATH="$PWD/spikes/sdk/python"
export N4_BIN="$PWD/spikes/engine/target/debug/n4"
mkdir -p .local/examples
cp -R spikes/examples/signal-atlas .local/examples/atlas-edit
```

Use a fresh destination for the copy. Change the title and measure how it wraps before saving:

```python
from n4 import Project

project = Project.open(".local/examples/atlas-edit")
survey = project.sequence("survey")
title = survey.layer("Film title")
title["content"]["text"] = "SIGNAL\nFIELD"
for line in survey.metrics("Film title")["lines"]:
    print(line["left"], line["right"], line["baseline"])
survey.write()
```

Both survey placements show the saved title. The master still owns the separate cuts and timings. To create a new label, import `label`, supply a project-relative font, set its position and append it with `sequence.add`. A recipe can use `encode_sequence(document)` to emit XML directly. A saved project can render independently of its generator. [Authoring format](../../../docs/project-format.md) · [Example artwork](../../examples/signal-atlas/sequences/survey.xml)

`project.details` lists the placed leaf definitions. `sequence.layer(*names)` follows direct children and reports a missing or ambiguous name. `sequence.document` exposes the full editable detail. `sequence.validate()` tests pending changes against every placement before saving. A write updates one detail atomically, preserves original bytes for a no-op, and detects intervening writes by cooperating clients. For XML containing comments or processing instructions, the SDK attempts an unambiguous edit and reports a conflict if it cannot preserve the source.

For use outside the checkout, install the package into your virtual environment with `python3 -m pip install ./spikes/sdk/python` and supply `N4_BIN` or the `n4=` argument to `Project.open`. Run the native-backed regression suite from the repository root with the environment above.

```sh
python3 -B -m unittest discover -s spikes/sdk/python/tests
```
