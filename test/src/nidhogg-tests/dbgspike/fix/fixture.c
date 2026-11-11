#include <stdio.h>

int main(void) {
  int total = 0;
  int items[] = {3, 1, 4, 1, 5};
  for (int i = 0; i < 5; i++) {
    total += items[i];
  }
  printf("total %d\n", total);
  return 0;
}
