// A small line chart of recent prices.
export function Sparkline({ values, width = 120, height = 34 }: { values: bigint[]; width?: number; height?: number }) {
  if (values.length < 2) return <svg className="spark" viewBox={`0 0 ${width} ${height}`} />;
  const low = values.reduce((a, b) => (b < a ? b : a));
  const high = values.reduce((a, b) => (b > a ? b : a));
  const range = high - low || 1n;
  const points = values
    .map((v, i) => {
      const x = (i / (values.length - 1)) * width;
      const y = height - (Number(((v - low) * 1000n) / range) / 1000) * height;
      return `${x.toFixed(1)},${y.toFixed(1)}`;
    })
    .join(" ");
  const down = values[values.length - 1] < values[0];
  const color = down ? "var(--bad)" : "var(--ok)";
  return (
    <svg className="spark" viewBox={`0 0 ${width} ${height}`} preserveAspectRatio="none">
      <polyline fill="none" stroke={color} strokeWidth="1.5" points={points} />
    </svg>
  );
}
