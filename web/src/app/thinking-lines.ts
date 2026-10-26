/** Decorative status copy; never represents real tool operations or model reasoning. */
export const THINKING_LINES = [
  "Reticulating splines… oh… I mean, doing whatever you just told me…",
  "Turning caffeine into plausible next tokens…",
  "Consulting the committee of approximately a trillion parameters…",
  "Checking whether the obvious solution is secretly the complicated one…",
  "Putting the ducks in a row. One of them is a compiler…",
  "Untangling the headphones of logic…",
  "Asking the rubber duck a follow-up question…",
  "Moving this from ‘probably’ toward ‘let’s check’…",
  "Finding the semicolon that holds the universe together…",
  "Giving the edge cases their own little helmets…",
  "Teaching the solution to survive contact with reality…",
  "Looking busy, but with significantly more matrix multiplication…",
  "Herding cats. The cats have strong opinions about variable names…",
  "Dividing by zero very carefully. Actually, let’s not…",
  "Convincing the parentheses to close ranks…",
  "Looking for a solution that doesn’t require its own onboarding course…",
  "Rearranging the mental furniture. Mind the recursion…",
  "Turning ‘it depends’ into something you can actually use…",
  "Waiting for the last brain cell to finish its pull request…",
  "Putting the ‘why’ in ‘why is this like this?’…",
  "Counting to ten in binary. This may look suspiciously quick…",
  "Assembling an answer. Some reasoning required…",
  "Checking the map before confidently walking into a lake…",
  "Giving Occam’s razor a quick sharpening…",
  "Negotiating a peace treaty between theory and practice…",
  "Following the breadcrumbs. Someone has replaced them with TODOs…",
  "Checking the fine print on ‘this should be easy’…",
  "Polishing the answer until the hand-waving comes off…",
  "Searching for the elegant solution behind this pile of clever ones…",
  "Taking the scenic route around an off-by-one error…",
  "Asking future me whether this is going to be embarrassing…",
  "Making a small plan before it becomes a large incident…",
  "Trying the door marked ‘simple explanation’ before the trapdoor…",
  "Teaching a stack of matrices some bedside manner…",
  "Untying a knot that appears to be load-bearing…",
  "Checking whether the light at the end of the tunnel is a train…",
  "Sorting the useful ideas from the ones wearing fake mustaches…",
  "Consulting the internal whiteboard. The markers are all imaginary…",
  "Looking for the missing piece. It’s probably under the assumptions…",
  "Giving the obvious answer a slightly less obvious inspection…",
  "Converting a vague hunch into a respectable sequence of steps…",
  "Trying to make the explanation shorter than the problem…",
  "Checking that the silver bullet isn’t just a very shiny footgun…",
  "Keeping the plot while the subplots attempt a takeover…",
  "Packing the answer with enough context, but no packing peanuts…",
  "Rehearsing the explanation for a rubber duck with high standards…",
  "Making sure the shortcut doesn’t lead to a longer problem…",
  "Putting a tiny seatbelt on the first assumption…",
  "Waiting for the idea to stop wobbling before calling it a solution…",
  "Checking the answer’s pockets for hidden dependencies…",
  "Attempting a graceful dismount from this train of thought…",
  "Translating enthusiastic hand gestures into actual instructions…",
] as const;
export const THINKING_LINE_INTERVAL_MS = 12_000;

interface ThinkingScheduler { setTimeout(callback: () => void, delay: number): number; clearTimeout(timer: number): void }

export class ThinkingLines {
  #timer: ReturnType<typeof setTimeout> | null = null;
  #key: string | null = null;
  #index = 0;
  #lines: readonly string[] = THINKING_LINES;
  constructor(readonly changed: () => void, readonly scheduler: ThinkingScheduler = globalThis) {}
  current(key: string | null, reducedMotion: boolean, lines: readonly string[] = THINKING_LINES): string | null {
    if (lines !== this.#lines) { this.dispose(); this.#lines = lines; }
    if (!lines.length) key = null;
    if (key !== this.#key) { this.dispose(); this.#key = key; }
    if (key === null) return null;
    if (reducedMotion && this.#timer !== null) { this.scheduler.clearTimeout(this.#timer); this.#timer = null; }
    if (!reducedMotion && this.#timer === null) {
      this.#timer = this.scheduler.setTimeout(() => {
        this.#timer = null;
        if (this.#key !== key) return;
        this.#index = (this.#index + 1) % this.#lines.length;
        this.changed();
      }, THINKING_LINE_INTERVAL_MS);
    }
    return this.#lines[this.#index] ?? null;
  }
  dispose(): void { if (this.#timer !== null) this.scheduler.clearTimeout(this.#timer); this.#timer = null; this.#key = null; this.#index = 0; }
}
