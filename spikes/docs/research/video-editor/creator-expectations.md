# Creator expectations and v1 acceptance

Proposed release criteria. The intended creator is one person making a video they care enough to revise precisely. They may edit directly, delegate work to an agent, or move between the two. The expectations below describe a candidate v1 and the evidence needed to call it shipped.

## The promise

Read the [creator walkthrough](blackbox.md) for an example cut a creator can revise directly in text and extend with a custom source or filter.

I can bring footage from my own drive, turn it into a polished video, and keep editing the work the agent did for me. If the last six frames of a cut feel wrong, I can fix those six frames. If a name changes after I approve an animated title, I can correct it without rebuilding the title. Tomorrow I can reopen the project, make another revision, and deliver again.

The two starting audiences are product creators and independent media producers, including documentary and channel editors. A founder can replace an application recording and retime the callout to the new interaction. A producer cutting a narrated explainer can move a chart reveal to a different word and keep its labels legible. Each can show a viewer the revision in the finished sequence. A professional editor should recognize the behavior of a trim, a keyframe, an audio edit, and undo. Feature-length work is the longer-term ambition. For v1, publish the project sizes, media profiles and machines on which the experience has actually been demonstrated.

## One production that should survive v1

Use real footage with visible motion and audible sync cues. Include a variable-frame-rate screen recording, a spoken take, and a graphic with a transparent edge. Keep the source files and the editable project with the review so another person can repeat it.

1. **Make the first cut.** I install the desktop app, import the recordings, find a moment by scrubbing, and trim two shots. I lower an audio clip so the spoken line is clear. I can hear the next line before the picture cuts. Extending the shot reveals the original footage up to the available source limit. The sound remains where I put it.
2. **Delegate a graphic.** I ask the agent to add an animated lower third. I see a title, its text, its placement, and its entrance as editable parts of my project. I change the name by hand and move the end of the entrance by six frames. I can undo and redo those changes.
3. **Revise the composition.** I set the title’s text width and alignment, check how the longer name wraps, and adjust its backplate by hand if needed. I move the title on the fixed canvas. Its entrance still lands on the chosen frame and the adjacent logo stays where I placed it.
4. **Delegate again.** I save my title correction and can see which lines changed in the project file. My agent finds the opening clip by ID and shortens it directly in the text. I can scrub the updated cut in the open editor and undo the accepted revision in one step. My corrected name and entrance timing survive. If an external edit overlaps an unsaved change, I can see both before choosing what to keep.
5. **Return to the work.** I save, quit, and reopen. My trims, audio edit, graphic and animation are still editable. If I move an original recording, the project identifies the missing media. I can relink it once and recover every use of it without recreating the cut.
6. **Deliver what I approved.** I can play the result and export a shareable SDR movie with sound. The file plays in an independent player. The cut lands on the approved frame, dialogue stays in sync, the title has the same line breaks and placement, and its soft edges have no visible fringe. If I edited with proxies, delivery uses the available originals. A missing original is surfaced before I accidentally deliver a proxy-quality master.

## What must be shown before calling it v1

| Creator expectation                              | Evidence to retain for release review                                                                                                                                                                                                                                                                                                                                |
| ------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| I can install the app and open my own footage.   | Install the packaged candidate on a clean supported machine. Publish the OS, hardware floor, and tested input/output profiles, including an example of unsupported media and the message or proxy option the creator sees. A filename extension alone is insufficient.                                                                                               |
| The editor keeps up with my hands.               | Scrub rapidly in both directions, zoom the timeline while thumbnails arrive, release on an identifiable frame, then play through an edit. Report cold and warm time to the correct frame, interaction latency, missed presentation deadlines, audio underruns and drift, and memory over repeated seeks. Observe the interaction on the actual display and speakers. |
| Edits are precise and reversible.                | Keep the before/after project and reference frames for the split picture/sound edit, six-frame timing correction and agent revision. Repeat the manual edit during a pending agent change. Confirm undo/redo, save/reopen and relink preserve the intended result.                                                                                                   |
| Graphics stay editable after ordinary revisions. | Inspect short and long titles at rest and mid-entrance. Verify the chosen line breaks, alignment, placement and transparent edges. Adjust a finish on one graphic, then change its text or path and confirm that its finish and timing remain editable.                                                                                                              |
| The file I send matches the work I approved.     | Check the exported frame count, cut frames, title timing, audio sync marks and color interpretation in an independent decoder/player. Compare known reference frames before compression and visually review the delivered file for expected compression differences. Record export time and any fallback used.                                                       |

**Performance sign-off still needs numbers.** Before testing a release candidate, agree on the baseline machine, footage resolution and frame rate, project length and layer count, output preset, and budgets for interaction latency, seek latency, missed presentation deadlines and memory. Specify allowed audio drift at the sync marks and require exact picture-frame identity at each test cut. Record those choices with the results. A smooth demo on an unspecified workstation cannot settle the release decision.

## Where the v1 promise ends

For design review, try revising a [shared editorial map, kinetic headline or product reveal](production-recipes.md). Promotion into the v1 acceptance table is a separate decision.

The proposed delivery target is one qualified desktop configuration and one dependable shareable export, initially Rec. 709 H.264/AAC MP4. Richer promises deserve their own acceptance examples. For caption interchange, prove a corrected cue survives sidecar export. For linked 3D, prove replacing the render preserves the cut. For a professional master, prove the working pipeline preserves its advertised precision and alpha. For feature-length work, qualify a representative long project.

Those examples can expand the release when evidence is available. The deciding v1 question is whether a creator can **revise the agent's work, trust what they see and hear, and deliver the result** from the installed product.
