import React from "react";
import { useCurrentFrame } from "remotion";
import {
  BEAT,
  C,
  MONO,
  RULE,
  SANS,
  drift,
  enter,
  line,
  sceneLength,
} from "../theme";
import { Logo, Stage } from "../ui";

/** Close-up end card: the name, the one command, the address. The last frame is complete. */
export const Install: React.FC = () => {
  const frame = useCurrentFrame();
  const length = sceneLength("install");
  const name = enter(frame, 0, 16);
  const brew = enter(frame, 12, 18);
  const url = enter(frame, 24, 18);
  const caretOn = Math.floor(frame / (BEAT / 2)) % 2 === 0;
  const bar = 1 - 0.08 * Math.sin((frame / BEAT) * Math.PI) ** 2;
  return (
    <Stage scale={drift(frame, length, 0.02)}>
      <div
        style={{
          position: "absolute",
          left: 200,
          top: 210,
          display: "flex",
          alignItems: "center",
          gap: 48,
          opacity: name,
        }}
      >
        <Logo size={220} bar={bar} />
        <div
          style={{
            fontFamily: SANS,
            fontSize: 168,
            fontWeight: 800,
            letterSpacing: "-0.04em",
            color: C.ink,
          }}
        >
          {line("title.name")}
        </div>
      </div>
      <div
        style={{
          position: "absolute",
          left: 200,
          top: 540,
          padding: "26px 40px",
          border: `3px solid ${RULE}`,
          fontFamily: MONO,
          fontSize: 60,
          color: C.ink,
          opacity: brew,
          transform: `translateY(${(1 - brew) * 24}px)`,
        }}
      >
        {line("install.brew")}
        <span
          style={{
            display: "inline-block",
            width: "0.6ch",
            height: 60,
            marginLeft: 12,
            verticalAlign: "-0.12em",
            backgroundColor: C.lime,
            opacity: caretOn ? 1 : 0,
          }}
        />
      </div>
      <div
        style={{
          position: "absolute",
          left: 204,
          top: 760,
          fontFamily: SANS,
          fontSize: 64,
          fontWeight: 600,
          color: C.lime,
          opacity: url,
          transform: `translateY(${(1 - url) * 20}px)`,
        }}
      >
        {line("install.url")}
      </div>
    </Stage>
  );
};
