# Local AI training

tinyBird now has a Gymnasium environment and a local Stable-Baselines3 PPO/DQN
runner. Both use the existing headless subprocess; they do not expose a web API,
run Python in the website, or require a cloud account. The basic emulator and
Python client still work without training dependencies.

## Install

```sh
cargo build --locked --release -p tinybird-runtime
python -m venv target/training-venv
# Windows:
target/training-venv/Scripts/python.exe -m pip install -r examples/python/requirements-training.txt
# Linux/macOS: use target/training-venv/bin/python instead.
```

Commands below use `python`; run them with that environment's Python executable.
On Windows append `.exe` to the headless executable path.

## Try without a commercial game

```sh
python examples/python/make_training_demo.py
python examples/python/train.py target/training-demo/buttons.gba target/training-demo/observations.json target/training-demo/task.json --executable target/release/tinybird-headless --steps 1024 --eval-episodes 10
```

This generated original ARM test cartridge reports success when RIGHT is pressed.
It validates the emulator-to-agent interface; it is intentionally trivial and is
not evidence of general game-playing ability. Run its contract tests with:

```sh
python examples/python/test_training.py
```

## Mario: first-screen experiment

The included task is for **Super Mario Advance 4, AX4P revision 0**, using the
user-provided world-map checkpoint tested during development. It presses A for
180 frames, waits 60 frames, then captures an in-process baseline at the start
of level 1. Each reset restores that baseline, so menu time is paid once per
environment, not every episode. For a checkpoint already inside the level,
remove `reset_sequence` and verify the initial fields before training.

```sh
python examples/python/train.py "your-mario.gba" examples/python/tasks/mario-ax4p-first-screen.observations.json examples/python/tasks/mario-ax4p-first-screen.task.json --state "your-start.state" --executable target/release/tinybird-headless --steps 10000 --seed 7 --eval-episodes 10 --record
```

No ROM, BIOS, state, or trained model is committed. Use a raw **Save to file**
state, not a compressed Vault container. If your baseline needs a BIOS, supply
`--bios`. The tested baseline rendered correctly without an additional BIOS
argument; that does not establish BIOS-free compatibility with other baselines.

The first goal is **world X >= 200 pixels**, beyond the first enemy, not finishing
the level. Failure is world Y >= 420 pixels, observed during the death/fall
sequence. Episodes truncate after 120 actions (480 game frames). Six actions
cover waiting, right, right+jump, right+run, right+run+jump, and left. The policy
receives four recent pairs of normalized world X/Y observations, not pixels.
The runtime still renders; this is not a rendering-free fast path.

Fields were discovered and cross-checked on that ROM and starting state:

| Name | Address | Width | Observed behavior |
| --- | --- | --- | --- |
| `player.world_x_fixed` | `0x03003F24` | 4 | Fixed point, 256 units/pixel. Initial 6144 (24 px); RIGHT at 8/16/24 frames: 6576/7888/10112; LEFT at 24: 2096; waiting: 6144. Continues past screen-camera movement. |
| `player.world_y_fixed` | `0x03003F28` | 4 | Initial 98304 (384 px); jump after 16 frames: 86784 (339 px). Walking into the first enemy leads to Y 424 at frame 216 and 448 at frame 224. |

These are observations for the tested opening section, not a fully researched
Mario memory map. A different revision, hack, state, level, or game mode needs
validation. The death threshold is specific to this section, not a general lives
counter. Do not extend it to the whole level without researching deaths, level
completion, enemy state, camera/world boundaries and relevant feature ranges.

## Task contract

Supply two JSON files: the existing Workshop observation export, and a training
task. The runtime enforces the observation game's header/revision. The task names
only fields in that export:

- `actions`: a discrete list of valid GBA button combinations; no START/SELECT
  unless the task actually needs them.
- `frame_skip`: 1–600 emulator frames per action; `max_steps` bounds an episode.
- `release_frames`: optional 0–10 frames with no buttons after an action. This
  creates an actual release between repeated jump commands. It adds emulated
  time per step; a terminal event after the held portion skips the release.
- `features`: field/min/max definitions normalized and clipped to [0,1].
  `info.out_of_range` reports values outside those bounds. Inspect them when
  selecting a new game; clipping does not validate a mistaken address.
- `history`: recent feature vectors stacked into one observation.
- `rewards`: `delta` pays signed changes; `progress` pays only improvements beyond
  the episode's best scaled value, avoiding repeat rewards for backtracking.
  Negative scale can reward reducing a distance. `step_reward` supplies a small
  time cost when desired.
- `terminal`: ordered field comparisons (`eq`, `lt`, `le`, `gt`, `ge`), an outcome
  (`success`/`failure`) and a one-time reward. Put failures first if rules overlap.
- `reset_sequence`: optional fixed bootstrap actions, cached in slot 15.
  `reset_choices` and `reset_frames` optionally choose a seeded startup action
  after restoring the baseline. They do not change the emulator's RNG directly.

Task files contain no expressions, arbitrary code, network calls, or Python
imports. Missing fields and malformed configurations fail before training.
Terminal starting states are rejected. Emulator failures are surfaced; the
caller must reset before stepping again, because a failed frame may have partly
advanced. Do not disguise emulator errors as successful episodes.

## Results and evaluation

Each new run creates a new directory under `target/training` containing:

- `manifest.json`: ROM/BIOS/state/runtime SHA-256 fingerprints, exact task and
  observations, seed and installed library versions.
- `episodes.monitor.csv`: training episode rewards and lengths.
- `model.zip`: selected local policy weights; `final_model.zip`: final training weights.
- `best/` and `validation/`: best deterministic validation checkpoint and scores, checked every 512 actions.
- `checkpoints/`: periodic recovery weights.
- `evaluation.json`: random-policy and trained-policy results on matching seeds.
- `evaluation.gif` with `--record`: the first evaluation episode, capped at 1000
  images. The evaluation itself continues to its terminal/time-limit condition.

```sh
python examples/python/train.py your.gba observations.json task.json --state start.state --executable target/release/tinybird-headless --evaluate target/training/YOUR_RUN --eval-episodes 20 --record
```

To continue training into a fresh output directory, replace `--evaluate` with
`--resume target/training/YOUR_RUN --steps 4096`. This retains policy/optimizer
state and adds training steps; it starts fresh episodes and reseeds the sampler,
so it is not a bit-for-bit continuation of an interrupted episode.

Evaluation rejects changed game files, runtime binaries, observations, or tasks.
Add `--stochastic` to evaluate seeded samples from the learned action distribution
instead of the default most-likely action. The manifest and results record this
choice. Report the two modes separately; successful exploratory training episodes
do not establish that deterministic playback works.
Load only your own trusted local model files. PPO collects full 256-action
rollouts, so requested training steps round up to a rollout boundary. Runs use
one CPU worker and one Torch thread; they do not train indefinitely. Close the
process to stop; completed periodic checkpoints remain. Mid-run checkpoints can
be loaded through SB3 directly; the CLI resumes/evaluates a run's selected model.
DQN resumes weights and optimizer state with a fresh replay buffer. It is not
an exact continuation of the previous sampling process.

Use `--algorithm dqn` to compare DQN with the default PPO. The separate
`mario-ax4p-first-screen-dqn.task.json` profile narrows actions to RIGHT and
RIGHT+A, allowing the agent to learn when to press and release jump without
redundant running combinations. Both algorithms can use this profile. Its
success/failure rewards are stronger than the original task's. Compare success
rates across profiles; reward totals have different meanings.

Training selects between the best validation checkpoint and final weights using
deterministic validation reward, then evaluates the selected model on separate
evaluation seeds. On a fixed starting state, different seeds alone do not create
different deterministic trajectories.

## Visible playback

After a run finishes, open the selected model in a live desktop window:

```sh
python examples/python/watch_agent.py target/training/YOUR_RUN
```

The window offers Pause, Restart and Close and loops completed episodes with a
short pause. This is live deterministic inference, not a recording. It needs
Tkinter and Pillow. The run manifest records absolute local input paths; playback
checks ROM, BIOS, state and runtime hashes and uses the saved task definitions.
Keep those inputs available at their recorded locations. Older runs without
input paths must first be evaluated with the updated runner to create a new
manifest and a copy of the selected model.

Run `python examples/python/benchmark_agent.py target/training/YOUR_RUN` to
compare the policy with random actions and every constant action at eight startup
delays. Results go to `delayed-starts.json`. These delays change enemy timing;
they still cover only one checkpoint and one short section of Mario.

With the initial Mario task, all evaluation episodes start at the same checkpoint
and the deterministic policy produces the same trajectory. Repeated success
measures repeatability, not generalization. Before claiming a robust Mario agent,
evaluate held-out checkpoints, varied startup delays and multiple training seeds.
Compare against simple scripted policies as well as random actions. Next steps
are enemy/ground observations, proper death/goal flags, richer starting states,
longer sections, and later pixel policies or parallel workers.

The initial 6,144-action Mario run exposed that distinction: the most-likely
action was continuously RIGHT+A+B. Mario jumped too early, never released jump
for a second jump, and hit the first enemy. Preserve that failure when comparing
future changes. Better timing observations, training, or an explicitly defined
press/release action space are needed for reliable deterministic playback.

An experimental `mario-ax4p-first-screen-pulse.task.json` adds one release frame
after each four-frame action. It is a separate task, so previous models cannot
silently resume against changed action semantics. Holding its run+jump action
also failed the first enemy in the initial scripted check; it is not presented
as a trained or improved solution.

## Initial measured results

| Experiment | Training actions | Evaluation |
| --- | --- | --- |
| Generated button task, seed 3 | 512 | Trained 6/6 successes; random 4/6. |
| Mario raw actions, seed 7 | 2,048 | Deterministic 0/6; random 3/6. |
| Mario continued, seed 7 | 6,144 total | Deterministic 0/6; random 3/6. |
| Same Mario model, sampled policy, evaluation seed 101 | No additional training | Sampled 7/12; random 10/12. |

These small, single-checkpoint measurements validate training and evaluation
plumbing. They do not demonstrate an improved Mario agent. Models, episode logs,
manifests and evaluation GIFs are kept under `target/training` locally.

## Improved timing experiment

The simplified action profile was trained with DQN for 8,192 actions, seed 23,
and compared with PPO on the same profile. DQN's checkpoint at action 2,560
learned to run to about x=99 before jumping, release jump, then press it again
near the enemy. Later DQN checkpoints regressed; the runner retains the earlier
successful checkpoint rather than automatically playing the last weights.
The selected DQN model passed 20/20 fixed-start evaluation episodes, compared
with random actions at 11/20. PPO trained for 4,096 actions on the same profile
passed 0/20. Repeated fixed-start episodes demonstrate reproducibility, not
20 distinct gameplay situations.

That checkpoint passed all eight delayed-start cases (0, 4, 8, 16, 24, 32, 48
and 64 additional neutral frames). Random inputs passed 18/24 episodes across
those delays; holding RIGHT or RIGHT+A continuously each passed 0/8. These are
small measurements on the first-screen task, not evidence of completing a level
or handling other checkpoints. The next useful training expansion is varied
starting states and longer goals with verified death/goal observations.

## Score-aware full-level attempts

Use `mario-ax4p-level-1.observations.json` with `mario-ax4p-level-1.task.json`
for attempts through world 1-1 from the same starting state:

```sh
python examples/python/train.py "your-mario.gba" examples/python/tasks/mario-ax4p-level-1.observations.json examples/python/tasks/mario-ax4p-level-1.task.json --state "your-start.state" --executable target/release/tinybird-headless --algorithm dqn --steps 8192 --seed 31 --eval-episodes 3
```

This is a new task with different inputs and actions; train it into a new run
directory. The old first-screen weights cannot be resumed with this profile.
X now covers 0–4096 pixels, Y covers 0–512, and power-up state is included.
Actions include walking, running, jumping and leftward movement. Episodes allow
up to 18,000 emulated frames. Reaching x=200 no longer ends an attempt.

- Score: `0x03003854`, unsigned 32-bit, stores displayed points divided by ten.
  The first enemy stomp was verified to change this from 0 to 10 while the HUD
  showed 100 points. It earns +0.25 reward. Only new score highs within the
  episode earn points; unchanged values and recovery to an earlier high do not.
- Forward progress: +0.01 per new pixel beyond the episode's previous best X.
- Time: -0.002 per action. No new pixel of progress for 180 actions ends a
  stalled attempt with -5. Death also gives -5.
- Completion: +50 on course-clear music 0x1D or 0x20. These correspond to the
  normal goal-panel and 1-up clear routines. They are source-backed signals,
  not an arbitrary distance threshold. Normal course-clear music 0x1D was positively verified on September 10
  by collecting the goal card, followed by the score bonus and return to the map.
  The alternate 0x20 clear music remains source-backed only.

The music field is unsigned 16-bit at `0x03000098`. Normal level music 0x10
and death music 0x24 were verified against this European checkpoint. Unlike the
first-screen profile, full-level death detection does not assume that a fixed
Y coordinate means death. The score/music definitions and goal-panel behavior
were cross-checked with the [SMA4 disassembly and its RAM documentation](https://github.com/KarisaAdvynia/sma4-disasm).
That reference targets a US revision; the European ROM still needs a positive
alternate clear-music check before claiming every completion variant is validated.

Task `metrics` entries map observation fields to scaled display values. Playback
shows actual score separately from training reward. `evaluation.json` records
final metrics, peak metrics, metric gains and reward contributions per episode.
The generic `stall` setting specifies a progress field, minimum improvement,
action limit and optional terminal penalty. Stalls are truncations; a game
terminal event on the same action takes precedence.

The first completed full-level run used 8,192 DQN actions, seed 31. Its selected
checkpoint (action 5,120) earned 100 points and reached a peak X of 339.94 before
stalling in all three deterministic evaluations. The score contribution was
exactly +0.25, with no repeated reward while standing still. It cleared 0/3
levels. Artifacts are in `target/training/mario-full-level-v2`; v1 was stopped
early to correct the missing stall penalty and is not a completed experiment.
Separate scripted task-verification probes reached X=1856 but did not reach the
goal. Those are not learned-policy results. The next improvement should expose
nearby enemies and obstacles, then expand training with varied starting states.

## Hybrid visual network

`mario-ax4p-hybrid.task.json` enables visual observations and jump pulses. It uses
the full-level observation definitions and goals:

```sh
python examples/python/train.py "your-mario.gba" examples/python/tasks/mario-ax4p-level-1.observations.json examples/python/tasks/mario-ax4p-hybrid.task.json --state "your-start.state" --executable target/release/tinybird-headless --algorithm ppo --steps 4096 --seed 41 --eval-episodes 6
python examples/python/watch_agent.py target/training/YOUR_RUN --start-variant 0
python -m unittest discover -s examples/python -p "test_*.py"
```

The network receives a dictionary with four 96×64 grayscale frames (`image`,
uint8 channels-first) and four normalized RAM observations (`ram`, X/Y/power-up).
Three convolutional layers with 16/32/32 channels preserve spatial information,
then project the image features to 128 values. A separate RAM encoder outputs
64 values. Their concatenation feeds PPO's actor and value networks, each with
128/64 hidden units. SB3 normalizes the image bytes to [0,1]. Frame stacks are
reset to the starting image every episode, so previous attempts cannot leak in.

The visual policy uses four PPO optimization epochs, entropy regularization and
256-action rollouts. DQN is also supported; its visual replay buffer is capped
at 4,096 transitions (about 192 MiB for the two image arrays), rather than using
the RAM-only buffer size. `network.txt` records the instantiated architecture;
the run manifest identifies the network and hashes `hybrid_policy.py` alongside
the environment and trainer. Existing RAM-only models retain their old inputs.

`--device auto` uses CUDA when the installed PyTorch build can access it, otherwise
CPU. Use `--device cpu` or `--device cuda` to choose explicitly. Playback loads
weights on CPU. GPU training requires a CUDA-enabled PyTorch wheel and compatible
driver; follow the [official PyTorch installation selector](https://pytorch.org/get-started/locally/).
Verify with `python -c "import torch; print(torch.cuda.is_available())"`.
Emulation still runs on CPU, so GPU support alone does not remove the environment
throughput bottleneck. Prefer a release build of the headless runtime for long
runs. Changing the runtime binary requires a new experiment signature.

Each action holds buttons for 16 frames, then releases only A for one real
emulated frame while preserving movement and B. Repeated jump actions can
therefore jump again after landing. Actions are run right, run+jump right,
run left, run+jump left, and wait. This timing is part of the model's task
signature, not a playback-only fix.

`reset_variants` defines optional input sequences after the common startup.
They are replayed once and cached into separate in-process checkpoints. This
profile samples the level start and a pipe approach at about X=289, assembled
from a previous local policy's inputs. Existing points at that checkpoint do
not earn a new score reward. `reset(options={'variant':0})` or the viewer's
`--start-variant 0` selects the level start; index 1 selects the pipe approach.
Evaluation records each starting variant. Compare them separately, since they
represent different tasks and difficulty.
The checkpoint callback reseeds its starting-state sequence for each validation.
For delayed-start comparisons, `benchmark_agent.py --start-variant 0` fixes all
policies to the level start; use index 1 to compare the pipe approach.

Playback is explicitly labeled as not learning. Its **Agent view** checkbox
shows the exact four grayscale frames supplied to the policy, oldest to newest.
The checkpoint selector switches between the saved starts; for the Mario profile,
Checkpoint 1 is the level start and Checkpoint 2 is the pipe approach.
Tests verify both network branches receive gradients, changing images changes
features, models save/reload, image stacks reset, and A releases without dropping
RIGHT. They require the optional training dependencies.

In a controlled check, constantly selecting run+jump with the new release timing
reached X=1507.69 and earned 200 points from the level start, but died at X=350.69
from the pipe approach. This is a scripted diagnostic, not a trained-network
result. It demonstrates that release handling matters and that blindly jumping
can still hit the Piranha Plant. The goal-clear signal still needs positive
live validation; neither adding a CNN nor this diagnostic proves level mastery.

### Demonstration warm-start

The optional `mario-ax4p-hybrid.demonstrations.json` contains 42 verified input
decisions: a surviving opening prefix and a seven-action pipe sequence that
reached X=466.625 in a controlled search. The latter includes a short wait for
the plant encounter. These are demonstrations, not trained-policy results.

Add `--demonstrations examples/python/tasks/mario-ax4p-hybrid.demonstrations.json`
to a PPO training command, optionally with `--resume YOUR_RUN`. The runner
replays the inputs through the real emulator to collect image/RAM observations,
rejects failing or timed-out demonstrations, and fits the actor with supervised
cross-entropy before reinforcement learning. It records the exact demonstrations
in the manifest and training action accuracy in `demonstrations.json`.
That accuracy measures fit to examples, not game-playing generalization.

`initial_model.zip` preserves the warm-started actor. Final selection compares
that model, the best validation checkpoint, and final weights using matching
evaluation seeds. If reinforcement learning regresses, the earlier model remains
available. A demonstration warm-start is supported only for PPO.

The first hybrid run completed 4,096 PPO actions. From the level start its selected
policy reached X=1507.69 with 200 points in three deterministic evaluations,
matching the constant-jump diagnostic. From the pipe checkpoint it died at
X=350.69 in three evaluations. It cleared no full levels. This is why the pipe
demonstration is needed; a working CNN input path alone does not establish that
the policy has learned to handle the encounter.

The guided follow-up replayed the 42 demonstration decisions, achieved 100%
training action accuracy, and ran another 512 PPO actions on the RTX 3060 using
PyTorch 2.11.0+cu128. Reinforcement-learning refinement regressed, so selection
kept `initial_model.zip`, the demonstration-warmed network. Its final evaluation
reached X=1507.69 with 200 points from the level start (three episodes), and
X=1797.375 with 300 points from the pipe checkpoint (one episode). Both passed
the original pipe; neither cleared the entire level. The pipe checkpoint starts
with 100 points already, so its additional score was 200. Artifacts are under
`target/training/mario-hybrid-guided-v1`. This is a demonstration-assisted policy,
not a claim that PPO alone learned the pipe encounter.
Additional CPU playback checks with 0/4/8 extra neutral starting frames passed
the pipe in all three level-start cases, but only the zero-delay pipe-approach
case. The delayed pipe cases died around X=351. Blanking the image input at the
zero-delay pipe checkpoint also caused failure at X=350.69, with RAM still
present. Images therefore affect the learned behavior, but the encounter is
still sensitive to timing. The next training expansion should add varied enemy
phases and more successful trajectories, not merely run this fixed playback
longer. Detailed results are in the run's `timing-checks.json`.

The implementation follows SB3's [multiple-input custom feature extractor](https://stable-baselines3.readthedocs.io/en/master/guide/custom_policy.html#multiple-inputs-and-dictionary-observations)
interface. The adapter follows [Gymnasium's reset/step API](https://gymnasium.farama.org/api/env/)
and uses normalized features and separate evaluation as recommended in
[SB3's training guidance](https://stable-baselines3.readthedocs.io/en/master/guide/rl_tips.html).

### Balanced Mario curriculum

`examples/python/tasks/mario-ax4p-curriculum.task.json` adds three starts: the opening,
first pipe, and the approach to the later hole. Its companion `.demonstrations.json`
contains surviving input sequences for all three. These are local demonstrations,
not proof that a trained policy completes the level.

The task keeps the five previous actions and adds four-frame walking, four-frame
running jumps, four-frame leftward braking, and a 24-frame upward jump. Every action
still releases A for one emulated frame. `action_frames` supplies individual action
durations; old tasks continue using `frame_skip`.

Rewards now distinguish the objectives:

- Course-clear signal: +100; death: -15. The clear music values remain source-backed
  with normal clear music now positively verified by collecting the goal card.
- Each newly earned 100 HUD points: +1, capped at +20 per episode. Starting score,
  unchanged score, and recovering a previous score do not pay again.
- Forward progress: +0.01 per new pixel, capped at +40 per episode.
- Time: -0.02 per emulated second, with a 300-second episode budget. Seconds use
  the adapter's nominal 60 Hz clock, not wall-clock training time or the HUD timer.

`frame_reward` applies to frames actually requested, including jump-release frames;
`max_frames` clips the final action to the remaining budget. Terminal checks happen
at action boundaries. Evaluation reports elapsed frames/seconds, earned score, peak
position, outcome, and separate reward terms. Playback displays elapsed seconds and
paces each action using its actual duration.

Model selection now evaluates every configured start rather than sampling a few
possibly identical starts. It ranks completion rate first, then the weakest start's
net forward progress, then balanced reward (which includes points and time). This
protects regional progress but is not a guarantee against every kind of regression.
`--rehearsal-epochs 50` creates a separate candidate by revisiting demonstrations
after PPO. Initial, final, intermediate-best, and rehearsed weights remain available;
only the candidate that wins the same evaluation suite becomes `model.zip`.

Example: use the normal local ROM/state arguments with
`mario-ax4p-level-1.observations.json`, `mario-ax4p-curriculum.task.json`,
`--demonstrations examples/python/tasks/mario-ax4p-curriculum.demonstrations.json`
and `--rehearsal-epochs 50 --steps 512 --device cuda`. Create a new output directory;
the expanded action space is incompatible with directly resuming an older model.
Short pilot runs measure whether the setup helps; they are not sufficient evidence
of reliable full-level play. No ROM, savestate, or learned weights belong in Git.

For transfer learning, `--initialize-from PREVIOUS_RUN` copies compatible PPO
weights with a fresh optimizer. It verifies the ROM, state, runtime, observation
layout, and every existing action's buttons and duration. Extra actions are allowed;
reinterpreting existing actions is rejected. The source model hash is recorded.
`--demonstration-epochs 1000` allows a longer supervised warm-start when a short one
fails to learn the demonstrations. Training-set accuracy alone does not establish
successful gameplay or generalization.

#### September 10 curriculum pilots

Two 512-step CUDA pilots were completed. The first, trained from scratch, regressed
at the first pipe. The second initialized from `mario-hybrid-guided-v1`, learned all
46 demonstration actions with 1,000 supervised epochs, then ran PPO and 100 epochs
of rehearsal. Selection retained `rehearsed_model.zip` as `model.zip`.

| Start | Previous peak x | Selected peak x | Points earned, previous / selected | Selected outcome |
| --- | ---: | ---: | ---: | --- |
| Opening | 1,508 | 1,636 | 200 / 200 | Death after 13.0 s |
| First pipe | 1,797 | 2,800 | 200 / 200 | Stalled after 32.4 s |
| Hole approach | 1,508 | 1,855 | 100 / 0 | Death after 4.63 s |

The pipe checkpoint begins with 100 points, so its final HUD score is 300. Longer
survival is not a faster completion: none of these episodes cleared the course.
The hole demonstration taught a route around the old failure but sacrificed the
nearby enemy points. A held-out four-frame delay produced opening x910/0 points,
pipe x351/100 total points, and hole x2084/100 total points. This remains sensitive
to timing and is not a robust whole-level policy.

Local evidence lives under `target/training/mario-curriculum-v2/`: `selection.json`,
`comparison.json`, `evaluation.json`, and `timing-check.json`. Models and reports
are ignored local artifacts. The displayed playback uses checkpoint 2 (the first
pipe) to show the furthest learned route; select checkpoint 1 for the full opening.

The pilots also exposed a large PPO policy shift. New visual PPO runs now default
to learning rate 0.0001 and a target KL of 0.03 to stop overly large update passes.
These conservative defaults passed a separate synthetic visual training smoke run;
the Mario pilot results above used the previous 0.0003 learning rate and no KL limit.
Existing resumed models retain their saved hyperparameters.

### Goal card and completion score

`mario-ax4p-course.task.json` adds a fourth, near-goal start; its demonstrations
include a complete route from the first-pipe checkpoint and the jump into the goal
card. Collecting the card positively verified normal clear music 29 (0x1D). A
separate neutral-input replay then awarded the remaining-time bonus and returned
to the world map. Alternate clear music 32 remains source-backed only.

Training/evaluation stop at the card: their score is the **score at the goal**, not
the later automatic tally. In the verification replay, 300 points became 14,150
points after the 13,850-point remaining-time bonus. The +100 completion reward
credits finishing; the per-frame cost rewards using less play time. Automatic
movement after finishing does not earn forward-progress reward or count as play
 time. The capped point reward applies to points earned before the goal.

Playback accepts `--finish-animation-frames 300` to show the automatic score tally
before restarting. It keeps play time fixed at the goal and shows the updated score
from RAM. This makes the final game score visible without confusing celebration
frames with the agent's decisions. The agent-view strip remains the last frames
actually observed by the policy during play.

The follow-up `mario-course-v1` pilot completed 256 PPO steps using the conservative
settings. Its selected model is the policy initialized from the previous run and
trained on 106 demonstrated transitions for 1,000 supervised epochs. Selection kept
this pre-PPO candidate: later PPO and rehearsal did not improve the whole suite.
Its `model_timesteps` is zero because that counter tracks PPO rollouts, not copied
training or supervised updates; the manifest records its provenance and demos.

| Start | Selected outcome | Score at endpoint | Play time |
| --- | --- | ---: | ---: |
| Full opening | Death at x1,636 | 200 | 13.0 s |
| First pipe | Course clear | 300 (200 earned since checkpoint) | 18.92 s |
| Hole approach | Death at x1,855 | 100 (0 earned since checkpoint) | 4.63 s |
| Goal approach | Course clear | 300 | 1.10 s |

These are checkpoint-assisted clears, not successful play from the level's initial
state. With four extra neutral reset frames, the opening, pipe, and hole starts
failed; only the goal start still cleared. Exact-start imitation and timing
sensitivity remain the principal limitations. Evidence is in the run's
`selection.json`, `evaluation.json`, and `timing-check.json`. The final visible
playback starts at the first pipe and shows the full finishing animation/tally.

### Productive pauses and stable policy updates

`mario-ax4p-collect.observations.json` and `mario-ax4p-collect.task.json` are the
current points-and-coins profile. `mario-ax4p-score.task.json` is the intermediate
score-only profile retained for the pilot comparison.

- Fresh score or fresh level coins reset the inactivity timer, as does new forward
  progress. The timer measures 720 emulated frames (12 nominal seconds), independent
  of action duration. Remaining stationary while collecting is valid play. The
  overall 300-second episode limit still applies.
- Score pays +1 per newly earned 100 HUD points, without the old +20 episode cap.
  Coins pay +0.25 each, separately from score. Completion remains +100, and play time
  costs 0.02 per nominal second. The point/coin counters use episode high-water
  marks so falling back to an old value does not pay twice or reset inactivity.
- The level coin counter at 0x030037A0 was verified against Mario's wallet at
  0x03002C58 during playback: both rose from 1 to 2 while score stayed at 100. Thus
  coin collection previously received no reward in that transition. The one-byte
  level counter can wrap at 256; high-water reward does not yet count coins beyond
  that wrap. These first-level pilots do not reach that limit.
- Selection ranks completion rate, then the combined points/coins/progress/time
  reward, then remaining progress ties. A productive detour can beat a farther run
  with lower return. Evaluation samples starting checkpoints in round-robin order;
  choose a multiple of four episodes for equal coverage of this profile.

New visual PPO policies use independent actor and critic feature encoders. Value
prediction loss therefore cannot change the actor's visual or RAM encoder. This
addresses a concrete failure mode in the shared model; it does not guarantee
learning or accurate value estimates. A gradient-isolation test verifies that a
critic-only update leaves actor logits exactly unchanged.

`mario-ax4p-score.demonstrations.json` uses label smoothing 0.05 so supervised
training does not demand probability 1 for every demonstrated action. The
rehearsal step uses the same targets. This permits more exploration while retaining
the demonstrations' preferred actions. Actor/critic isolation, checkpoint transfer,
productive inactivity, reward-aware ranking, and balanced evaluation are covered by
16 passing tests.

Use `--initialize-from target/training/mario-course-v2` for the first run with this
profile and network, plus the collect observations/task and score demonstrations.
Unlike resume, initialization builds a fresh optimizer and separate encoders.
It permits extra reward/diagnostic observation fields but rejects changes to any
existing memory field, policy input, or action meaning. After that, resume a run
created with the new profile to preserve its architecture and optimizer.

#### Productive-play pilot results

Two CUDA pilots completed: `mario-score-v1` (512 steps) and `mario-collect-v1`
(256 steps). Both retained their final PPO candidate; their deterministic routes
matched their post-demonstration initial candidates. Thus these short pilots show
preserved behavior after PPO, not an improvement caused by PPO itself.

The score-only pilot clears from the full opening in 21.6 nominal seconds with 200
points at the goal, and retains the pipe clear. This is the best full-opening model
and is running in playback. Its logged first PPO update had approximate KL 0.008
and entropy loss -1.27, versus the prior saturated policy and frequent large-KL
stops. Value prediction is still inaccurate, and this is only a short stability
check. With four extra neutral starting frames, opening and pipe still fail; the
near-goal start clears. Full-opening reliability across timings is not established.

The explicit-coin pilot reaches x1635 with 400 points from the opening but dies. It
retains the 18.92-second pipe clear with 300 total points and 2 total level coins
(200 points and 1 coin earned since that checkpoint). Its coin reward term is
+0.25. The hole start still fails in both pilots. All original runs remain intact.

For a longer run with coin rewards, initialize from the stronger `mario-score-v1`
policy without another demonstration warm-start. This preserves that candidate for
comparison and adds the coin observation/reward without altering policy inputs.
From the repository root, this PowerShell array avoids continuation-backtick issues:

```powershell
$previous = Get-Content .\target\training\mario-score-v1\manifest.json -Raw | ConvertFrom-Json
$trainingArgs = @(
  '.\examples\python\train.py', $previous.inputs.rom,
  '.\examples\python\tasks\mario-ax4p-collect.observations.json',
  '.\examples\python\tasks\mario-ax4p-collect.task.json',
  '--executable', $previous.inputs.executable,
  '--state', $previous.inputs.state,
  '--initialize-from', '.\target\training\mario-score-v1',
  '--steps', '4096', '--seed', '57', '--device', 'cuda',
  '--eval-episodes', '8', '--output', '.\target\training\mario-collect-v2'
)
& .\target\training-venv\Scripts\python.exe @trainingArgs
```

Use a fresh output directory. `selection.json` covers all four starts;
`evaluation.json` reports each start separately and includes earned coins. To watch
the strongest completed pilot:

```powershell
& .\target\training-venv\Scripts\python.exe .\examples\python\watch_agent.py .\target\training\mario-score-v1 --start-variant 0 --agent-view --finish-animation-frames 300
```

### Preserve 400 points, then escape the jump loop

The `mario-400-v1` experiment starts from the user's newer `mario-course-v2-5`
model. It does not initialize from or launch the older full-clearing policy.
`mario-ax4p-400.task.json` uses the collect observations and adds a checkpoint just
after the model has earned 400 points at x1427. Its demonstrations replace the old
opening example with the full 38-action collection prefix followed by a searched,
verified escape. That combined 48-action replay reaches x1763 with all 400 points;
the same escape from the added checkpoint reaches x1757 with 400 points. These are
demonstration results, distinct from learned-policy evaluation.

The task's `evaluation_requirements` sets a minimum score gain of 400 for variant 0.
Candidate selection checks this requirement before ranking completion and reward,
so a 200-point clear cannot silently replace the intended collection behavior.
`selection.json` records the observed gains and whether each candidate passes.
Initialization/resume also saves `source_model.zip` before demonstration training;
it is evaluated alongside initial, final, intermediate-best, and rehearsed models.
This protects the chosen newer model against both imitation and PPO regressions.

Productive pauses continue to reset the 720-frame inactivity timer. Only an episode
with no fresh score, coins, or forward progress for that window receives the
inactivity termination penalty (-15 in this profile). The stronger timeout penalty
removes the incentive to repeat an empty jump indefinitely to avoid a death penalty;
it does not apply while collection continues. Goal, score, coin, and time rewards
otherwise match the collect profile.

The pilot uses separate actor/critic encoders, 0.05 label smoothing, 800 supervised
epochs, 512 CUDA PPO steps, and 100 rehearsal epochs. Its source is explicitly
`mario-course-v2-5`. No playback changes are part of this experiment.

#### 400-point experiment outcome

`mario-400-v1` completed its 512-step CUDA pilot and selected `initial_model.zip`
as `model.zip`: this is the newer source policy trained on the new 133-transition
collection-and-escape demonstrations for 800 supervised epochs. The later PPO
candidate regressed; rehearsal restored the clear but did not outperform the
initial candidate. Selection kept the stronger learned candidate automatically.

| Start | Source result | Selected new-model result |
| --- | --- | --- |
| Full opening | 400 points; stalled at x1428 | 400 points; course clear in 22.88 s |
| First pipe | Course clear, 300 total points | Same clear in 18.92 s |
| Hole approach | Death at x1855 | Still dies at x1855 |
| Goal approach | Course clear | Course clear |
| Immediately after 400 | Stalled at x1427 | Exits loop; dies later at x1792 |

Scores above are gameplay scores before the automatic finishing bonus. This is a
clear from the supplied full-opening state while preserving the 400-point behavior;
it is not a claim of robustness across arbitrary timings or every checkpoint.
No playback was opened or switched during this experiment.

17 unit tests pass. A separate generated-ROM CLI check verified that an impossible
score requirement writes its failed selection report but does not promote
`model.zip`. Future CLI runs also print their source/task/architecture and save
`initial_validation.json` before PPO so regressions are easier to diagnose.

To continue this specific new model, use the same collect observations and 400 task
with `--resume target/training/mario-400-v1`, a fresh output directory, and an
evaluation episode count divisible by five. This retains the new architecture,
400-point requirement, and optimizer. Do not reuse the course-task command for this
run; the task signatures intentionally differ. Example from the repository root:

```powershell
$previous = Get-Content .\target\training\mario-400-v1\manifest.json -Raw | ConvertFrom-Json
$trainingArgs = @(
  '.\examples\python\train.py', $previous.inputs.rom,
  '.\examples\python\tasks\mario-ax4p-collect.observations.json',
  '.\examples\python\tasks\mario-ax4p-400.task.json',
  '--executable', $previous.inputs.executable, '--state', $previous.inputs.state,
  '--resume', '.\target\training\mario-400-v1',
  '--steps', '4096', '--seed', '59', '--device', 'cuda',
  '--eval-episodes', '10', '--output', '.\target\training\mario-400-v2'
)
& .\target\training-venv\Scripts\python.exe @trainingArgs
```

## Training successive stages

`advance_stage.py` verifies a trained policy from reset variant 0 (the full stage
opening), requires a successful clear and any opening score floor, waits for the
world map, and follows a declarative map route. It checks the destination's RAM
stage ID, playing mode, and starting position. Death, inactivity, insufficient
points, a map timeout, or the wrong destination prevents checkpoint creation.

The checkpoint is a compressed input replay from your original local save file,
not a redistributed ROM or binary save. A new environment replays the preceding
stages once, then caches the next stage for fast episode resets. Longer courses
therefore take longer to initialize. Each reset variant supports up to 16,384
input steps, each limited to 600 frames. ROM, original save, and runtime hashes
remain checked by the existing training loader.

Stage 1 ? 2 has been verified with `mario-400-v1`: 400 gameplay points and a clear
in 22.88 seconds. The finishing timer bonus brings the carried score to 14,100.
Stage 2 starts at x24 with that score; rewards and evaluation gains count only
points earned after its reset. The previous stage's 400-point evaluation floor
is removed for the new stage; coin, score, completion, productive-pause, and time
rewards carry over. Add a stage-specific score floor once a useful collection
route is learned.

From the repository root, to reproduce the stage 2 preparation in a **new** directory:

```powershell
$python = '.\target\training-venv\Scripts\python.exe'
$unlockArgs = @(
  '.\examples\python\advance_stage.py',
  '--run', '.\target\training\mario-400-v1',
  '--route', '.\examples\python\tasks\mario-ax4p-stage-1-to-2.route.json',
  '--output', '.\target\training\mario-stage-2-new'
)
& $python @unlockArgs
```

The prepared directory contains `task.json`, `observations.json`, `start.png`,
`unlock.json` (source model hash, clear evidence, destination state, and task
hashes), and `train.ps1`. All generated artifacts stay under ignored `target/`.
The script uses absolute quoted local paths, transfers the prior stage's policy
with a fresh optimizer, and trains 4,096 steps into a separate run. For the stage 2
checkpoint already prepared in this workspace:

```powershell
& .\target\training\mario-stage-2\train.ps1
```

A 512-step CUDA pilot is already saved in `target/training/mario-stage-2-pilot-v1`.
It did not improve the deterministic policy: all four evaluation episodes died
at x627.8125 after 5.65 seconds, earning zero new points. This is a training
baseline, not a stage clear. To continue that pilot instead of initializing again:

```powershell
$python = '.\target\training-venv\Scripts\python.exe'
$previous = Get-Content .\target\training\mario-stage-2-pilot-v1\manifest.json -Raw | ConvertFrom-Json
$trainingArgs = @(
  '.\examples\python\train.py', $previous.inputs.rom,
  $previous.inputs.observations, $previous.inputs.task,
  '--executable', $previous.inputs.executable, '--state', $previous.inputs.state,
  '--resume', '.\target\training\mario-stage-2-pilot-v1',
  '--steps', '4096', '--seed', '61', '--device', 'cuda',
  '--eval-episodes', '4', '--output', '.\target\training\mario-stage-2-v2'
)
& $python @trainingArgs
```

After that model can clear stage 2, request stage 3:

```powershell
$unlockArgs = @(
  '.\examples\python\advance_stage.py',
  '--run', '.\target\training\mario-stage-2-v2',
  '--route', '.\examples\python\tasks\mario-ax4p-stage-2-to-3.route.json',
  '--output', '.\target\training\mario-stage-3'
)
& $python @unlockArgs
```

The stage 2 to 3 route was subsequently verified with `mario-stage-2-enemy-v1`:
the selected policy cleared stage 2 with 300 new points in 24.22 seconds, reached
stage ID 12, and passed the next-stage replay/reset check. The prepared checkpoint
is `target/training/mario-stage-3`; its `train.ps1` starts a separate stage 3 run.
The route refuses to create a checkpoint if navigation lands elsewhere.
Later stages use the same workflow with their own map routes and
verified identifiers. This is an explicit, gated command sequence, not an
unattended campaign trainer. Keep each stage's champion: transferring weights to
a new stage does not prove that the new model retains the old stage's behavior.
