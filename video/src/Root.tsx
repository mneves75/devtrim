import React from "react";
import { Composition } from "remotion";
import { DevtrimDemo } from "./DevtrimDemo";
import { timing } from "./film/theme";

export const RemotionRoot: React.FC = () => {
  return (
    <>
      <Composition
        id="DevtrimDemo"
        component={DevtrimDemo}
        durationInFrames={timing.durationFrames}
        fps={timing.fps}
        width={timing.width}
        height={timing.height}
      />
    </>
  );
};
