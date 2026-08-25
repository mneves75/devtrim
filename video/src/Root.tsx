import React from "react";
import { Composition } from "remotion";
import { DevtrimDemo } from "./DevtrimDemo";

export const RemotionRoot: React.FC = () => {
  return (
    <>
      <Composition
        id="DevtrimDemo"
        component={DevtrimDemo}
        durationInFrames={360}
        fps={30}
        width={1920}
        height={1080}
      />
    </>
  );
};
