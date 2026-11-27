# Authoring a project

Open a directory containing `project.proj`, or pass the master file directly. The master describes the edit across the film. A visual detail describes the layers that produce a picture at a local time. Media and font paths in those details are relative to the project directory. A relocated project should carry its dependencies with it. [Working project](../spikes/examples/signal-atlas)

```text
project ident fps 24/1 size 960x540
[0,48) V1 intro
[8,32) V2 strap
[48,96) V1 reprise
  [0,24) V1 intro
  [24,48) V1 close
```

On the last two rows, indentation places the children inside `reprise`. A film frame of 55 requests local frame 7 of the second occurrence of `intro`. Editing `sequences/intro.xml` affects both uses. Editing a placement in the master can change just that instance. Names accept ASCII letters, digits, underscores and hyphens. A `#` starts a comment.

| Row | Meaning in this alpha |
| --- | --- |
| `V1` | The base picture. Its intervals cover each scope continuously. Two spaces of indentation introduce children in parent-relative frames. A leaf reads its named detail. |
| `V2` | A graphics placement above the base picture in that scope. It can cross a base-picture cut. A root `track V2 off` directive after the header temporarily disables the root overlays. |
| `A1` | An audio placement. Its `sequences/<id>.json` detail identifies the source. The engine maps project-frame boundaries onto the stereo mix sample clock. |
| `CAM` | A cut to a named camera in the enclosing V1 leaf. Camera cuts use picture-local time and cover that placement. |

Intervals are half-open integer project frames. The start is visible and the end belongs to the next interval. At 24/1 fps, `[0,48)` lasts two seconds. The header also accepts rational rates such as `24000/1001`. Each root or nested scope uses the project rate. Order rows by start frame within each scope. The format accepts one V1, V2 and A1 lane per scope. V2 and A1 may contain gaps. Placements on a single lane must be disjoint and end within the picture.

For example, `sequences/intro.xml` can grow a line across a card:

```xml
<sequence background="#14252a">
  <rect name="Rule" x="72" y="268" height="4" fill="#ffba64">
    <animate attributeName="width">
      <key frame="0" value="2"/>
      <key frame="23" value="816"/>
    </animate>
  </rect>
</sequence>
```

A visual leaf owns paint order, transforms, masks, filters and local keyframes. A sequence may have many visual layers inside one timeline placement. Text wraps to its authored width using the supplied font. An imported SVG contributes one drawable. The alpha reader also accepts JSON visual details for engine fixtures. A detail ID must resolve unambiguously. [XML vocabulary](../spikes/docs/research/video-editor/sequence-xml.md) · [Master reader](../spikes/engine/src/project/master.rs)

Run `just host-native`, then `spikes/engine/target/debug/n4 inspect spikes/examples/signal-atlas` to see how Rust resolved this project's occurrences and keys. Open the same directory with the [Python SDK](../spikes/sdk/python/README.md) to edit a named layer and validate it against its placements before writing.
