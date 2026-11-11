function main() {
  let total = 0;
  const items = [3, 1, 4, 1, 5];
  for (const x of items) {
    total += x;
  }
  console.log("total", total);
}
main();
