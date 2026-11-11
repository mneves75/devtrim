import React from "react";
import { useCurrentFrame } from "remotion";
import {
  C,
  MONO,
  PANEL,
  enter,
  keyCues,
  leave,
  line,
  sceneLength,
} from "../theme";
import { Keycap, Panel, PreviewTitle, Stage } from "../ui";
import { PreviewPanel } from "./PreviewPanel";

const keys = keyCues("confirm");
const at = (key: string) => keys.filter((k) => k.key === key)[0].at;

const TOP = 70;
const PREVIEW_FONT = 76;
const PREVIEW_LINE = 104;
const FONT = 64;
const LINE_HEIGHT = 96;
/** The box sits just inside the cleared panel: its widest line is 39 cells. */
const INSET = 20;

/** The TUI's key line: each key lime, its word in ink, the separator muted. */
const KeyLine: React.FC<{ text: string }> = ({ text }) => (
  <>
    {text.split(/( · )/).map((part, i) => {
      if (i % 2 === 1)
        return (
          <span key={i} style={{ color: C.muted }}>
            {part}
          </span>
        );
      const [key, ...rest] = part.split(/(?= )/);
      return (
        <span key={i}>
          <span style={{ color: C.lime, fontWeight: 700 }}>{key}</span>
          {rest.join("")}
        </span>
      );
    })}
  </>
);

/**
 * Close-up: the narrowed plan holds a beat; a clears the body behind its title,
 * as the TUI does, and raises the confirmation; y approves it.
 */
export const Confirm: React.FC = () => {
  const frame = useCurrentFrame();
  const length = sceneLength("confirm");
  const a = at("a");
  const y = at("y");
  // The body clears first, so no row shows through the box as it rises.
  const clear = enter(frame, a, 8);
  const raise = enter(frame, a + 4, 14);
  const collapse = leave(frame, y + 48, 12);
  const lineIn = (n: number) => enter(frame, a + 10 + n * 8, 14);
  return (
    <Stage scale={1 + (0.02 * frame) / length} origin="50% 45%">
      {/* The plan Select ended on; only its title row stays once a is pressed. */}
      <div style={{ position: "absolute", left: PANEL.left, top: TOP }}>
        <PreviewPanel
          part="start"
          font={PREVIEW_FONT}
          lineHeight={PREVIEW_LINE}
          frame={frame}
          cursor={2}
          thirdSelected={0}
          narrowed={1}
        />
        <div
          style={{
            position: "absolute",
            left: -8,
            right: -8,
            top: PREVIEW_LINE,
            bottom: -8,
            backgroundColor: C.bg,
            opacity: clear,
          }}
        />
      </div>
      <div
        style={{
          position: "absolute",
          left: PANEL.left + INSET,
          top: TOP + PREVIEW_LINE * 1.4,
          opacity: raise * collapse,
          transform: `translateY(${(1 - raise) * 48 + (1 - collapse) * 24}px)`,
        }}
      >
        <Panel
          title={
            <span style={{ opacity: lineIn(0) }}>
              <PreviewTitle text={line("confirm.title")} />
            </span>
          }
          width={PANEL.width - 2 * INSET}
          font={FONT}
          lineHeight={LINE_HEIGHT}
          rows={4}
        >
          <div style={{ color: C.red, fontWeight: 700, opacity: lineIn(1) }}>
            {line("confirm.warning")}
          </div>
          <div style={{ fontWeight: 700, opacity: lineIn(2) }}>
            {line("confirm.effect")}
          </div>
          <div style={{ opacity: lineIn(3) }}>{line("confirm.plan")}</div>
        </Panel>
      </div>
      <div
        style={{
          position: "absolute",
          left: PANEL.left,
          top: 760,
          fontFamily: MONO,
          fontSize: FONT,
          color: C.ink,
          whiteSpace: "pre",
          opacity: lineIn(4) * collapse,
        }}
      >
        <KeyLine text={line("confirm.footer")} />
      </div>
      <div
        style={{
          position: "absolute",
          right: 140,
          top: 820,
          display: "flex",
          gap: 32,
        }}
      >
        <Keycap label={line("key.a")} frame={frame} at={a} size={64} />
        <Keycap label={line("key.y")} frame={frame} at={y} size={64} />
      </div>
    </Stage>
  );
};
