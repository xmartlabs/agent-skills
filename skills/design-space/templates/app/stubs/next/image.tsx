import type { ImgHTMLAttributes } from "react";

type Props = Omit<ImgHTMLAttributes<HTMLImageElement>, "src"> & {
  src: string | { src: string };
  alt: string;
  // Next.js props that make no sense here; accepted and ignored.
  fill?: boolean;
  priority?: boolean;
  quality?: number;
  placeholder?: string;
  blurDataURL?: string;
  unoptimized?: boolean;
};

export default function Image({ src, fill, priority, quality, placeholder, blurDataURL, unoptimized, style, ...rest }: Props) {
  void priority; void quality; void placeholder; void blurDataURL; void unoptimized;
  const url = typeof src === "string" ? src : src.src;
  const fillStyle = fill ? { position: "absolute" as const, inset: 0, width: "100%", height: "100%" } : undefined;
  return <img src={url} style={{ ...fillStyle, ...style }} {...rest} />;
}
