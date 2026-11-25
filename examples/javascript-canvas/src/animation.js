(seconds) => {
  const pulse = Math.sin(seconds * 2.5);
  return seconds * 1.25 + pulse * 0.8;
}
