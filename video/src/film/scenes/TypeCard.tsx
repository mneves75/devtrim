import React from "react";
import { useCurrentFrame } from "remotion";
import {
  BEAT,
  C,
  SANS,
  drift,
  enter,
  leave,
  line,
  sceneLength,
} from "../theme";
import { Stage, Words } from "../ui";

/** Full-screen type, centred, with a lime rule drawn under it. */
export const See: React.FC = () => {
  const frame = useCurrentFrame();
  const length = sceneLength("see");
  const out = leave(frame, length);
  return (
    <Stage scale={drift(frame, length, 0.03)}>
      <div
        style={{
          position: "absolute",
          inset: 0,
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          justifyContent: "center",
          opacity: out,
          transform: `translateY(${(1 - out) * -24}px)`,
        }}
      >
        <Words
          text={line("see")}
          frame={frame}
          start={0}
          stagger={6}
          style={{
            fontSize: 132,
            fontWeight: 800,
            color: C.ink,
            letterSpacing: "-0.04em",
          }}
        />
        <div
          style={{
            marginTop: 36,
            height: 10,
            width: 820 * enter(frame, 24, BEAT),
            backgroundColor: C.lime,
          }}
        />
      </div>
    </Stage>
  );
};

/** Full-screen type, left-aligned on two lines: the method in four words. */
export const Approve: React.FC = () => {
  const frame = useCurrentFrame();
  const length = sceneLength("approve");
  const out = leave(frame, length);
  const [first, second] = line("approve").split(/(?<=\.) /);
  const style: React.CSSProperties = {
    fontFamily: SANS,
    fontSize: 168,
    fontWeight: 800,
    letterSpacing: "-0.045em",
    lineHeight: 1.05,
  };
  return (
    <Stage scale={drift(frame, length, 0.03)} origin="15% 50%">
      <div
        style={{
          position: "absolute",
          left: 160,
          top: 300,
          opacity: out,
          transform: `translateX(${(1 - out) * -40}px)`,
        }}
      >
        <Words
          text={first}
          frame={frame}
          start={0}
          stagger={6}
          style={{ ...style, color: C.ink }}
        />
        <Words
          text={second}
          frame={frame}
          start={BEAT}
          stagger={6}
          style={{ ...style, color: C.lime }}
        />
      </div>
    </Stage>
  );
};
