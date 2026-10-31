import { Easing, interpolate } from "remotion";
import copy from "../../film/copy.json";
import theme from "../../film/theme.json";
import timing from "../../film/timing.json";

/** The six colours; check_contrast.py reads the same file. */
export const C = theme.colours;
export { timing };

/** Borders use the muted colour itself, so the palette stays at six. */
export const RULE = theme.colours.muted;

export const SANS =
  'system-ui, -apple-system, "SF Pro Display", ui-sans-serif, sans-serif';
export const MONO = 'ui-monospace, "SF Mono", Menlo, monospace';

/** Entrances decelerate at reading speed; exits accelerate and are short. */
export const EASE_IN = Easing.bezier(0.16, 1, 0.3, 1);
export const EASE_OUT = Easing.bezier(0.7, 0, 0.84, 0);
export const EASE_MOVE = Easing.bezier(0.65, 0, 0.35, 1);

export const BEAT = timing.beatFrames;
export const ENTER = 18;
export const EXIT = 8;

/** Handoff geometry shared by every wide shot of the preview panel. */
export const PANEL = {
  left: 120,
  top: 150,
  width: 1680,
  font: 42,
  lineHeight: 64,
};
/** Nothing important below this line: the mobile player covers the bottom 8%. */
export const SAFE_BOTTOM = Math.floor(timing.height * 0.92);

type CopyId = keyof typeof copy;
/** Every on-screen string comes through here; check_facts.py traces each id to facts.json. */
export const line = (id: CopyId): string => copy[id].text;

export const sceneLength = (id: string): number => {
  const scene = timing.scenes.find((s) => s.id === id);
  if (!scene) throw new Error(`timing.json has no scene ${id}`);
  return scene.to - scene.from;
};

/** The key presses timing.json places inside a scene, in scene-local frames. */
export const keyCues = (id: string): { key: string; at: number }[] => {
  const scene = timing.scenes.find((s) => s.id === id);
  if (!scene) throw new Error(`timing.json has no scene ${id}`);
  const presses: { key: string; at: number }[] = [];
  for (const cue of timing.cues) {
    if (
      cue.key !== undefined &&
      cue.frame >= scene.from &&
      cue.frame < scene.to
    ) {
      presses.push({ key: cue.key, at: cue.frame - scene.from });
    }
  }
  return presses;
};

/** 0 → 1 over `duration` frames from `start`, decelerating. */
export const enter = (frame: number, start: number, duration = ENTER) =>
  interpolate(frame, [start, start + duration], [0, 1], {
    easing: EASE_IN,
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });

/** 1 → 0 over the last `duration` frames before `end`, accelerating. */
export const leave = (frame: number, end: number, duration = EXIT) =>
  interpolate(frame, [end - duration, end], [1, 0], {
    easing: EASE_OUT,
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });

/** A move between two values over `duration` frames, eased at both ends. */
export const move = (
  frame: number,
  start: number,
  from: number,
  to: number,
  duration = 10,
) =>
  interpolate(frame, [start, start + duration], [from, to], {
    easing: EASE_MOVE,
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });

/** The slow push every hold keeps, so no frame is still for long. */
export const drift = (frame: number, length: number, amount = 0.015) =>
  1 + (amount * frame) / length;
