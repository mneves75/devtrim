import React from "react";
import { useCurrentFrame } from "remotion";
import { C, SANS, drift, line, sceneLength } from "../theme";
import { Logo, Stage } from "../ui";

/** Frame 0 is the thumbnail: the name and the promise are already set. */
export const Title: React.FC = () => {
  const frame = useCurrentFrame();
  const length = sceneLength("title");
  const bar = 1 - 0.08 * Math.sin((frame / 30) * Math.PI) ** 2;
  return (
    <Stage scale={drift(frame, length, 0.02)} origin="20% 50%">
      <div
        style={{
          position: "absolute",
          left: 160,
          top: 260,
          display: "flex",
          alignItems: "center",
          gap: 36,
        }}
      >
        <Logo size={150} bar={bar} />
        <div
          style={{
            fontFamily: SANS,
            fontSize: 112,
            fontWeight: 700,
            color: C.ink,
            letterSpacing: "-0.03em",
          }}
        >
          {line("title.name")}
        </div>
      </div>
      <div
        style={{
          position: "absolute",
          left: 160,
          top: 500,
          width: 1500,
          fontFamily: SANS,
          fontSize: 132,
          fontWeight: 800,
          lineHeight: 1.02,
          color: C.ink,
          letterSpacing: `${-0.035 + (0.008 * frame) / length}em`,
        }}
      >
        {line("title.head")}
      </div>
      <div
        style={{
          position: "absolute",
          left: 164,
          top: 820,
          width: 220 + frame * 1.2,
          height: 10,
          backgroundColor: C.lime,
        }}
      />
    </Stage>
  );
};
