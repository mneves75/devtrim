import React from "react";
import { useCurrentFrame } from "remotion";
import {
  BEAT,
  C,
  MONO,
  RULE,
  enter,
  keyCues,
  leave,
  line,
  sceneLength,
} from "../theme";
import { Keycap, Stage } from "../ui";
import { PreviewPanel } from "./PreviewPanel";

const keys = keyCues("confirm");
const at = (key: string) => keys.filter((k) => k.key === key)[0].at;

const FONT = 44;
const LINE_HEIGHT = 70;

/** Close-up: a raises the confirmation for the exact plan; y approves it. */
export const Confirm: React.FC = () => {
  const frame = useCurrentFrame();
  const length = sceneLength("confirm");
  const a = at("a");
  const y = at("y");
  const raise = enter(frame, a, 14);
  const collapse = leave(frame, y + 24, 12);
  const lineIn = (n: number) => enter(frame, a + 6 + n * 8, 14);
  const caretOn = Math.floor(frame / (BEAT / 2)) % 2 === 0;
  return (
    <Stage scale={1.06 + (0.03 * frame) / length} origin="50% 45%">
      <div
        style={{
          position: "absolute",
          left: 120,
          top: 160,
          opacity: 0.22,
          transform: "scale(1.1)",
          transformOrigin: "0 0",
        }}
      >
        <PreviewPanel frame={frame} cursor={2} thirdSelected={0} narrowed={1} />
      </div>
      <div
        style={{
          position: "absolute",
          left: 250,
          top: 190,
          width: 1420,
          padding: "28px 48px 32px",
          border: `3px solid ${RULE}`,
          backgroundColor: C.bg,
          fontFamily: MONO,
          fontSize: FONT,
          lineHeight: `${LINE_HEIGHT}px`,
          color: C.ink,
          whiteSpace: "pre",
          opacity: raise * collapse,
          transform: `translateY(${(1 - raise) * 48 + (1 - collapse) * 24}px)`,
        }}
      >
        <div style={{ color: C.muted, opacity: lineIn(0) }}>
          {line("confirm.title")}
        </div>
        <div style={{ color: C.red, fontWeight: 700, opacity: lineIn(1) }}>
          {line("confirm.warning")}
        </div>
        <div style={{ opacity: lineIn(2) }}>{line("confirm.plan")}</div>
        <div style={{ color: C.amber, fontWeight: 700, opacity: lineIn(3) }}>
          {line("confirm.prompt")}
        </div>
        <div style={{ opacity: lineIn(4) }}>
          <span style={{ color: C.lime }}>{line("confirm.caret")}</span>
          <span style={{ display: "inline-block", width: "1ch" }} />
          <span
            style={{
              color: C.lime,
              fontWeight: 700,
              opacity: frame >= y ? 1 : 0,
            }}
          >
            {line("key.y")}
          </span>
          <span
            style={{
              display: "inline-block",
              width: "0.6ch",
              height: FONT,
              marginLeft: 4,
              verticalAlign: "-0.15em",
              backgroundColor: C.lime,
              opacity: caretOn ? 1 : 0,
            }}
          />
        </div>
        <div
          style={{ color: C.muted, fontSize: FONT * 0.72, opacity: lineIn(5) }}
        >
          {line("confirm.footer")}
        </div>
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
