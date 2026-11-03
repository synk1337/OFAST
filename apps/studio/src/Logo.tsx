// The OFAST logo in the header: the O with the wedge in its corner (the app icon's mark), then FAST, flat shapes
// on a grid where the letters are 100 units tall. The O takes the accent colour, the letters the text colour.

const MARK = [
  // The O: a ring.
  "M103 50C103 78.443 79.943 101.5 51.5 101.5C23.057 101.5 0 78.443 0 50C0 21.557 23.057 -1.5 51.5 -1.5C79.943 -1.5 103 21.557 103 50ZM79 50C79 65.188 66.688 77.5 51.5 77.5C36.312 77.5 24 65.188 24 50C24 34.812 36.312 22.5 51.5 22.5C66.688 22.5 79 34.812 79 50Z",
  // The wedge in its top-right corner.
  "M82.731 -1.5L103 -1.5L103 18.769Z",
];

const LETTERS = [
  "M113 0L171 0L171 20L136 20L136 39L163 39L163 58L136 58L136 100L113 100Z",
  "M165 100L199 0L217 0L251 100L226 100L219.2 80L196.8 80L190 100ZM208 47.059L213.42 63L202.58 63Z",
  "M315.901 14.814C308.831 1.518 293.174 -4.702 278.908 0.118C264.642 4.937 255.965 19.378 258.407 34.237C260.849 49.096 273.692 60 288.75 60C293.524 60 297.597 63.457 298.371 68.169C299.145 72.88 296.394 77.459 291.871 78.987C287.347 80.515 282.383 78.543 280.141 74.327L260.716 84.656C268.016 98.384 284.182 104.806 298.912 99.83C313.642 94.854 322.601 79.943 320.08 64.601C317.558 49.259 304.298 38 288.75 38C284.465 38 280.811 34.897 280.116 30.669C279.421 26.441 281.89 22.332 285.949 20.96C290.009 19.589 294.464 21.359 296.476 25.142Z",
  "M325.5 0L389.5 0L389.5 20L369 20L369 100L346 100L346 20L325.5 20Z",
];

const WIDTH = 389.5;
const HEIGHT = 103; // the O overshoots the letters by 1.5 above and below

/** `height`: in pixels; the width follows. */
export function Logo({ height = 16 }: { height?: number }) {
  return (
    <svg
      className="logo"
      viewBox={`0 -1.5 ${WIDTH} ${HEIGHT}`}
      height={height}
      width={(height * WIDTH) / HEIGHT}
      role="img"
      aria-label="OFAST"
    >
      {MARK.map((d) => (
        <path key={d} d={d} fill="var(--accent)" fillRule="evenodd" />
      ))}
      {LETTERS.map((d) => (
        <path key={d} d={d} fill="var(--text)" fillRule="evenodd" />
      ))}
    </svg>
  );
}
