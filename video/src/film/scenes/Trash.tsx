import React from "react";
import { interpolate, useCurrentFrame } from "remotion";
import {
  BEAT,
  C,
  EASE_OUT,
  PANEL,
  SANS,
  drift,
  enter,
  line,
  sceneLength,
} from "../theme";
import { Stage } from "../ui";
import { PreviewPanel } from "./PreviewPanel";

/** Each approved row leaves to the right, one per half beat once the returned inventory has held 0.75 s; the one left out stays. */
const away = (frame: number, start: number) =>
  interpolate(frame, [start, start + 18], [0, 2000], {
    easing: EASE_OUT,
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });

/** Wide: the approved findings move out; a caption says where they went. */
export const Trash: React.FC = () => {
  const frame = useCurrentFrame();
  const length = sceneLength("trash");
  const half = BEAT / 2;
  const offsets = [
    away(frame, 3 * half),
    away(frame, 4 * half),
    0,
    away(frame, 5 * half),
  ];
  const caption = enter(frame, BEAT, 20);
  const sub = enter(frame, BEAT + 10, 20);
  return (
    <Stage scale={drift(frame, length, 0.02)} origin="30% 50%">
      <div style={{ position: "absolute", left: PANEL.left, top: 60 }}>
        <PreviewPanel
          part="end"
          font={64}
          lineHeight={92}
          frame={frame}
          cursor={2}
          thirdSelected={0}
          narrowed={1}
          offsets={offsets}
        />
      </div>
      <div
        style={{ position: "absolute", left: 160, top: 680, fontFamily: SANS }}
      >
        <div
          style={{
            fontSize: 104,
            fontWeight: 800,
            letterSpacing: "-0.04em",
            color: C.ink,
            opacity: caption,
            transform: `translateY(${(1 - caption) * 30}px)`,
          }}
        >
          {line("trash.head")}
        </div>
        <div
          style={{
            marginTop: 18,
            fontSize: 80,
            fontWeight: 600,
            color: C.lime,
            opacity: sub,
            transform: `translateY(${(1 - sub) * 24}px)`,
          }}
        >
          {line("trash.sub")}
        </div>
      </div>
    </Stage>
  );
};
