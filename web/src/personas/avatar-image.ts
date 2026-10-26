import type { Persona } from "./schema.js";

/** Trusted animation templates; imported SVG cannot provide CSS or scripts. */
const patterns = {
  float: {duration: "4.8s", frames: "0%,100%{transform:translateY(0) rotate(0)}50%{transform:translateY(-14px) rotate(-0.6deg)}"},
  twitch: {duration: "5s", frames: "0%,88%,100%{transform:scale(1)}90%{transform:scale(.94,1.06)}94%{transform:scale(1.04,.96)}"},
  "sway-left": {duration: "3.6s", frames: "0%,100%{transform:skewX(0)}50%{transform:skewX(-3.5deg)}"},
  "sway-right": {duration: "3.6s", frames: "0%,100%{transform:skewX(0)}50%{transform:skewX(3.5deg)}"},
} as const;

/** Remains an isolated IMG. The host supplies only allowlisted transform animations. */
export function avatarImage(avatar: NonNullable<Persona["avatar"]>): string {
  const rules: string[] = [];
  for (const [index, animation] of (avatar.animations ?? []).entries()) {
    const {target, motion, origin} = animation, pattern = patterns[motion];
    // Defense in depth for programmatically constructed personas.
    if (!/^[A-Za-z][\w-]{0,63}$/u.test(target) || !Object.hasOwn(patterns, motion) ||
      origin.length !== 2 || origin.some(value => !Number.isFinite(value) || Math.abs(value) > 10000)) continue;
    const name = `persona-part-${index}`;
    rules.push(`@keyframes ${name}{${pattern.frames}}#${target}{transform-box:view-box;transform-origin:${origin[0]}px ${origin[1]}px;animation:${name} ${pattern.duration} ease-in-out infinite}`);
  }
  if (!rules.length) return avatar.svg;
  const doc = new DOMParser().parseFromString(avatar.svg, "image/svg+xml");
  const style = doc.createElementNS("http://www.w3.org/2000/svg", "style");
  style.textContent = `@media (prefers-reduced-motion:no-preference){${rules.join("")}}`;
  doc.documentElement.prepend(style);
  return new XMLSerializer().serializeToString(doc.documentElement);
}

export function avatarImageUrl(avatar: NonNullable<Persona["avatar"]>): string {
  return "data:image/svg+xml;charset=utf-8," + encodeURIComponent(avatarImage(avatar));
}
