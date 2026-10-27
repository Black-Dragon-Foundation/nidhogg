/* Nidhogg's stall watchdog: the park protocol's steps.

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

/* Stub (Task 4).  Task 8 writes the protocol's steps here as inline
   functions, shared by the monitor, the interleaving model and the
   real-thread harness (nidhogg/test/watchdog-park-test.sh); each takes
   its memory order from NH_WD_PARK_ORDER, so the model and the mutation
   check read the same code.  */

#ifndef NIDHOGG_WATCHDOG_PARK_H
#define NIDHOGG_WATCHDOG_PARK_H

#ifdef HAVE_NIDHOGG_WATCHDOG

#include "nidhogg-watchdog.h"

#define NH_WD_PARK_ORDER __ATOMIC_SEQ_CST

#endif /* HAVE_NIDHOGG_WATCHDOG */

#endif /* NIDHOGG_WATCHDOG_PARK_H */
