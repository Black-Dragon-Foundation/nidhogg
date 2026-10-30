/* The workers' wake protocol under ThreadSanitizer (workers-harness.sh
   --tsan wake): the completion channel's clear-then-re-check, with a
   producer appending exactly between the two, then two free-running
   producers and one consumer.  Exit 0 = no lost wake-up (and, under
   TSan, no race report).

Copyright (C) 2026 Black Dragon Foundation contributors.

This file is part of Nidhogg.

Nidhogg is free software: you can redistribute it and/or modify
it under the terms of the GNU General Public License as published by
the Free Software Foundation, either version 3 of the License, or (at
your option) any later version.

Nidhogg is distributed in the hope that it will be useful,
but WITHOUT ANY WARRANTY; without even the implied warranty of
MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
GNU General Public License for more details.

You should have received a copy of the GNU General Public License
along with Nidhogg.  If not, see <https://www.gnu.org/licenses/>.  */

#include "nidhogg-workers.c"

int
main (void)
{
  if (!nh_wk_race_open ())
    {
      fprintf (stderr, "wake: cannot open a wake fd\n");
      return 1;
    }
  bool level = nh_wk_race_level ();
  int gap = nh_wk_race_gap (100000);
  int stress = nh_wk_race_stress (1000000, 2);
  printf ("wake: level %s, gap races lost %d, free-running lost %d\n",
	  level ? "ok" : "WRONG", gap, stress);
  return !(level && gap == 0 && stress == 0);
}
