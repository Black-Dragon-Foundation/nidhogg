;;; gui-runner.el --- run the *-gui-tests.el files on a Nidhogg GUI frame  -*- lexical-binding: t -*-

;; Loaded by nidhogg/test/gui.sh (`nidhogg -Q --fullscreen -l gui-runner.el', or, with --daemon, by the
;; daemon and started from `server-after-make-frame-hook').  Environment:
;;   GLITNIR_GUI_SCALE   the monitor scale the run asked for (default 1)
;;   GLITNIR_GUI_SIZE    WxH of the virtual monitor in device pixels (for the fractional-scale check)
;;   GLITNIR_GUI_TESTS   directory holding the *-gui-tests.el files (default: this file's directory)
;;   GLITNIR_GUI_SELECT  ERT selector, an Emacs regexp (default: every test)
;;   GLITNIR_GUI_FILES   a glob (in GLITNIR_GUI_TESTS) of the test files to load (default: every *-gui-tests.el)
;;   GLITNIR_GUI_TAG     when set, only rows tagged TAG: the selector is (and REGEXP (tag TAG))
;;   GLITNIR_GUI_COMPOSITOR  sway, mutter or recorder (selects the scale rule and the known differences)
;;   GLITNIR_GUI_RECORDER    the recorder's control socket (recorder only)
;;   GLITNIR_GUI_RECORDER_BIN, GLITNIR_GUI_RECORDER_PROFILE  its binary and profile file, for a row's second recorder
;;   GLITNIR_GUI_KNOWN       the known-differences file (gui-known-differences.eld next to the tests)
;;   GLITNIR_GUI_ONLY        one row's name: the run is that row alone, when the selector takes it
;;   GLITNIR_GUI_DONE        a file of row names, one per line, that a previous session of the same run already
;;                           reported: they are not run again (a lane that ended a hung row starts a new session)
;; The rows run in a fixed order: by file in GLITNIR_GUI_FILES's order (a space-separated list of globs), then by
;; name; rows tagged `runs-last' after every other.  Rows tagged `parity' (compared with another build's output)
;; are counted apart in the summary line.
;; Prints every result to `external-debugging-output', then `kill-emacs' 0 (all as expected) or 1.
;; The tests start only once the first frame is visible and has been redisplayed, never inside `command-line'.

(require 'ert)
(require 'seq)

(defconst glitnir-gui--dir (file-name-directory (or load-file-name buffer-file-name)))

(defun glitnir-gui--say (fmt &rest args)
  "Print FMT with ARGS as one line on stderr, in one write.
The protocol trace (WAYLAND_DEBUG) shares stderr, and `external-debugging-output' writes a character at a time, so a
trace line could land inside a verdict and hide it from the rigs that grep for it."
  (let ((s (concat (apply #'format fmt args) "\n")))
    (condition-case nil
        (let ((coding-system-for-write 'utf-8-unix))
          (write-region s nil "/dev/stderr" t 'silent))
      (file-error (princ s #'external-debugging-output)))))

(defun glitnir-gui--scale ()
  (string-to-number (or (getenv "GLITNIR_GUI_SCALE") "1")))

(defun glitnir-gui--compositor ()
  "The compositor gui.sh runs on: \"sway\", \"mutter\" or \"recorder\" (mutter when unset)."
  (or (getenv "GLITNIR_GUI_COMPOSITOR") "mutter"))

(defun glitnir-gui--scale-ready-p ()
  "Non-nil once the selected frame sits at the asked scale and the monitor's device size, which the native shell reports."
  (and (< (abs (- (frame-scale-factor) (glitnir-gui--scale))) 0.001)
       ;; the monitors are read only when a size is asked: a shell may not serve them yet
       (let ((size (getenv "GLITNIR_GUI_SIZE")))
         (or (not size)
             (let ((geo (alist-get 'geometry (car (display-monitor-attributes-list)))))
               (equal (format "%dx%d" (nth 2 geo) (nth 3 geo)) size))))))

(defun glitnir-gui--wait-for-scale ()
  (let ((deadline (+ (float-time) 10)))
    (while (and (not (glitnir-gui--scale-ready-p)) (< (float-time) deadline))
      (sit-for 0.1))
    (unless (glitnir-gui--scale-ready-p)
      (glitnir-gui--say "gui-runner: scale %s not reached in 10 s (frame-scale-factor %s, monitors %S)"
                        (glitnir-gui--scale) (frame-scale-factor) (display-monitor-attributes-list))
      (kill-emacs 1))))

(defun glitnir-gui--place-frame ()
  "On NS, move the selected frame onto the display ns-gui.sh made (GLITNIR_GUI_FRAME is \"LEFT TOP COLS LINES\")."
  (let ((spec (getenv "GLITNIR_GUI_FRAME")))
    (when (and spec (not (string-empty-p spec)))
      (pcase-let ((`(,left ,top ,cols ,lines) (mapcar #'string-to-number (split-string spec))))
        (set-frame-size nil cols lines)
        (set-frame-position nil left top)))))

(defun glitnir-gui--say-frame (when)
  "On Naglfar, log WHEN: the selected frame as Emacs has it (visible, position, size) and as the shell has it
\(`nhmac--window-info': AppKit's frame, its screen, occlusion), with `nhmac-stats'.  Each fact's error is logged."
  (when (fboundp 'nhmac--window-info)
    (let ((fact (lambda (f) (condition-case err (funcall f) (error (list 'error err))))))
      (glitnir-gui--say "gui-runner: %s: visible %S position %S size %dx%d; window %S; stats %S"
                        when (frame-visible-p (selected-frame)) (frame-position)
                        (frame-pixel-width) (frame-pixel-height)
                        (funcall fact #'nhmac--window-info) (funcall fact #'nhmac-stats)))))

(defun glitnir-gui--say-startup ()
  "On Naglfar, log the shell's startup trace (`nhmac--startup-log'), one event a line, before the rows start."
  (when (fboundp 'nhmac--startup-log)
    (let ((log (condition-case err (funcall 'nhmac--startup-log) (error (list (format "error %S" err))))))
      (dolist (e log)
        (glitnir-gui--say "gui-runner: startup: %s" (if (stringp e) e (format "%S" e)))))))

(defun glitnir-gui--wait-for-first-paint ()
  "Wait (10 s at most) until Glitnir's frame-time record holds a paint of the selected frame.
Only the native shell records paints.  On Naglfar, wait (10 s at most) until the frame has drawn: nothing else turns
the event loop before the rows, so its window's size would otherwise reach Emacs only inside the first row."
  (cond
   ((and (fboundp 'glitnir-stats) (featurep 'nidhogg-wayland))
    (let ((deadline (+ (float-time) 10)))
      (while (and (< (or (plist-get (glitnir-stats) :count) 0) 1) (< (float-time) deadline))
        (redisplay t)
        (sit-for 0.1))))
   ((fboundp 'nhmac-stats)
    (let ((deadline (+ (float-time) 10)))
      (while (and (not (glitnir-gui--frame-drew-p)) (< (float-time) deadline))
        (sit-for 0.1))))))

;;; Waiting.  A row waits on what the frame, the presenter or the compositor reports: a deadline bounds a failure and
;;; never decides a pass.

(defun gui-runner-wait (pred &optional seconds)
  "Run the event loop until PRED returns non-nil, and return its value; nil after SECONDS (default 10).
Timers, process output and GTK run; a native frame's input stays unread until PRED reads it."
  (let ((deadline (+ (float-time) (or seconds 10))) (v nil))
    (while (and (not (setq v (funcall pred))) (< (float-time) deadline))
      (accept-process-output nil 0.01))
    v))

(defun gui-runner-read-all (&optional fn)
  "Read the events that have arrived, until none is left, and call FN (when given) on each one.  What has not arrived
yet is for a condition, or the recorder's `ping' before this call.  The read's timeout only ends an empty read: a
timeout of 0 reads nothing, and one that passes during the read's redisplay returns nil with input pending, which is
why `input-pending-p' decides."
  (let ((e nil))
    (while (or (setq e (read-event nil nil 0.01)) (input-pending-p))
      (when (and e fn) (funcall fn e)))))

(defun gui-runner-observe (seconds &optional tick)
  "Watch the session for SECONDS, with redisplay, timers and events running, calling TICK (when given) on each turn.
This is the span a row asserts something does not happen in, not a wait for something to happen: a span too short
can miss a bug, it cannot fail a good run.  Input stays in the queue."
  (let ((end (+ (float-time) seconds)))
    (while (< (float-time) end)
      (when tick (funcall tick))
      (sit-for (min 0.05 (max 0 (- end (float-time)))))
      (accept-process-output nil 0.01))))

(defun gui-runner-stimulus (seconds)
  "SECONDS of real time, with Lisp not reading input, as a row's stimulus: a key held while Lisp is busy, random gaps
between a row's steps, the quiet the code under test measures.  Never a wait for something to happen."
  (sleep-for seconds))

(defun gui-runner-sleeping-thread (seconds)
  "Start a Lisp thread that sleeps SECONDS, a row's stimulus: Lisp with a thread alive and waiting."
  (make-thread (lambda () (sleep-for seconds))))

;;; Grabs from the compositor: (gui-runner-screen-grab) is (W H BYTES), BYTES W*H BGRA pixels, on every compositor

(require 'cl-lib)
(require 'dbus nil t)                   ; not in the macOS build
(require 'json)
(require 'seq)
(require 'cl-lib)

(defun gui-runner-grab-find-block (grab pixel bw bh)
  "The (X . Y) of the first BW x BH block of PIXEL in GRAB (a `gui-runner-screen-grab' result), or nil.
PIXEL is a list of the colour's B, G, R bytes; alpha is ignored.  First means first in row-major order, so the
position is the block's top-left corner when the colour forms one rectangle.  (From the pgtk runner's helper.)"
  (let* ((w (nth 0 grab)) (h (nth 1 grab)) (bytes (nth 2 grab))
         (needle (apply #'unibyte-string pixel)) (start 0) found)
    (while (and (not found) (setq start (string-search needle bytes start)))
      (if (/= (% start 4) 0)
          (setq start (1+ start))
        (let* ((i (/ start 4)) (x (% i w)) (y (/ i w)))
          (if (and (<= (+ x bw) w) (<= (+ y bh) h)
                   (cl-loop for dy below bh
                            always (cl-loop for dx below bw
                                            always (let ((o (* 4 (+ x dx (* w (+ y dy))))))
                                                     (and (= (aref bytes o) (nth 0 pixel))
                                                          (= (aref bytes (+ o 1)) (nth 1 pixel))
                                                          (= (aref bytes (+ o 2)) (nth 2 pixel)))))))
              (setq found (cons x y))
            (setq start (1+ start))))))
    found))

(defmacro gui-runner-keeping-frame-font (&rest body)
  "Run BODY, which changes the selected frame's scale and puts it back; then give the frame its fontset back.
A scale change re-opens the frame's font at the new device size through a new fontset (fontset-auto1, auto2, ...),
as `set-frame-font' does, and the scale going back does not bring the old one back: every later row's `default'
would name the new one."
  (declare (indent 0) (debug t))
  (let ((fontset (make-symbol "fontset")))
    `(let ((,fontset (face-attribute 'default :fontset (selected-frame))))
       (unwind-protect
           (progn ,@body)
         (unless (equal (face-attribute 'default :fontset (selected-frame)) ,fontset)
           (set-frame-parameter nil 'font ,fontset)
           (redisplay t))))))

(defun gui-runner-recorder (plist)
  "Send PLIST as one JSON line to the recorder at $GLITNIR_GUI_RECORDER; return the parsed reply (a plist).
Signals `error' when the reply's :ok is false."
  (let* ((path (getenv "GLITNIR_GUI_RECORDER"))
         (reply "")
         (proc (make-network-process
                :name "nh-recorder-control" :family 'local :service path :coding 'utf-8
                :filter (lambda (_p s) (setq reply (concat reply s))))))
    (unwind-protect
        (progn
          (process-send-string proc (concat (json-serialize plist) "\n"))
          (let ((deadline (+ (float-time) 10)))
            (while (and (not (string-suffix-p "\n" reply)) (< (float-time) deadline))
              (accept-process-output proc 0.1)))
          (unless (string-suffix-p "\n" reply)
            (error "recorder: no reply to %S" plist))
          (let ((parsed (json-parse-string reply :object-type 'plist :array-type 'list)))
            (unless (eq (plist-get parsed :ok) t)
              (error "recorder: %s" (plist-get parsed :error)))
            parsed))
      (delete-process proc))))

(defun gui-runner-recorder-later (steps)
  "Send STEPS to the recorder from a batch Nidhogg, one after another; return its process.
For a row whose next call blocks with no Lisp running (a drag's loop, a dialog).  Each step is (WAIT . PLIST): WAIT is
seconds of stimulus, or `drag', which waits (10 s at most) until the recorder has logged a drag that was not there at
this call.  PLIST is then sent, and its reply goes to the process's buffer as one line."
  (let ((form `(progn
                 (defun later-ask (plist)
                   (let* ((reply "")
                          (p (make-network-process
                              :name "later" :family 'local :service ,(getenv "GLITNIR_GUI_RECORDER") :coding 'utf-8
                              :filter (lambda (_p out) (setq reply (concat reply out))))))
                     (process-send-string p (concat (json-serialize plist) "\n"))
                     (let ((deadline (+ (float-time) 10)))
                       (while (and (not (string-suffix-p "\n" reply)) (< (float-time) deadline))
                         (accept-process-output p 0.1)))
                     (delete-process p)
                     reply))
                 (defun later-drags ()
                   (length (plist-get (json-parse-string (later-ask '(:cmd "drag_log"))
                                                         :object-type 'plist :array-type 'list)
                                      :drags)))
                 (dolist (step ',steps)
                   (if (numberp (car step))
                       (sleep-for (car step))
                     (let ((deadline (+ (float-time) 10)))
                       (while (and (<= (later-drags) ,(length (plist-get (gui-runner-recorder '(:cmd "drag_log")) :drags)))
                                   (< (float-time) deadline))
                         (accept-process-output nil 0.02))))
                   (princ (later-ask (cdr step)))))))
    (make-process :name "nhwl-gui-later" :noquery t :connection-type 'pipe
                  :buffer (generate-new-buffer " *nhwl-gui-later*")
                  :command (list (expand-file-name invocation-name invocation-directory)
                                 "-Q" "--batch" "--eval" (prin1-to-string form)))))

;; The empty-commit invariant: a row of a recorder run fails when the recorder logged a commit that attached a new buffer
;; with no damage while the row ran.  On unless NHWL_GUI_STRICT_COMMITS is 0 (gui.sh --env).  A row that stops the
;; recorder (`display-lost-daemon-survives', and the rows after it) leaves nothing to ask: no control socket, no check.
(defun gui-runner--strict-commits-p ()
  (let ((ctl (getenv "GLITNIR_GUI_RECORDER")))
    (and (not (equal (getenv "NHWL_GUI_STRICT_COMMITS") "0")) ctl (file-exists-p ctl))))

(defun gui-runner--strict-commits (run test &rest args)
  "`ert-run-test' advice: add the empty-buffer-commit check to the end of TEST's body."
  (if (not (gui-runner--strict-commits-p))
      (apply run test args)
    (let ((body (ert-test-body test)))
      (setf (ert-test-body test)
            (lambda ()
              (let ((since (plist-get (gui-runner-recorder '(:cmd "log" :since 1000000000)) :next)))
                (funcall body)
                (let ((empty (and (gui-runner--strict-commits-p)
                                  (plist-get (gui-runner-recorder (list :cmd "empty_buffer_commits" :since since))
                                             :commits))))
                  (when empty
                    (ert-fail (list "buffer commits with no damage"
                                    (mapcar (lambda (c) (list :seq (plist-get c :seq) :role (plist-get c :role)))
                                            empty))))))))
      (unwind-protect (apply run test args)
        (setf (ert-test-body test) body)))))

(advice-add 'ert-run-test :around #'gui-runner--strict-commits)

;; The held-buffer invariant: a row of a recorder run fails when the recorder found a buffer whose bytes changed between
;; its attach and its release, with the release during the row.  On unless NHWL_GUI_STRICT_HELD is 0 (gui.sh --env).
(defun gui-runner--strict-held-p ()
  (let ((ctl (getenv "GLITNIR_GUI_RECORDER")))
    (and (not (equal (getenv "NHWL_GUI_STRICT_HELD") "0")) ctl (file-exists-p ctl))))

(defun gui-runner--strict-held (run test &rest args)
  "`ert-run-test' advice: add the held-buffer-write check to the end of TEST's body."
  (if (not (gui-runner--strict-held-p))
      (apply run test args)
    (let ((body (ert-test-body test)))
      (setf (ert-test-body test)
            (lambda ()
              (let ((since (plist-get (gui-runner-recorder '(:cmd "held_buffer_writes" :since 1000000000)) :next)))
                (funcall body)
                (let ((writes (and (gui-runner--strict-held-p)
                                   (plist-get (gui-runner-recorder (list :cmd "held_buffer_writes" :since since))
                                              :writes))))
                  (when writes
                    (ert-fail (list "buffers written to while attached"
                                    (mapcar (lambda (w) (list :surface (plist-get w :surface) :buffer (plist-get w :buffer)
                                                              :attached (plist-get w :attached_seq)
                                                              :released (plist-get w :released_seq)))
                                            writes))))))))
      (unwind-protect (apply run test args)
        (setf (ert-test-body test) body)))))

(advice-add 'ert-run-test :around #'gui-runner--strict-held)

(defvar gui-runner--pw-procs nil "The pipewire and wireplumber processes this session started.")
(defvar gui-runner--cast-session nil "The ScreenCast session's D-Bus path, held by Emacs's own connection.")

(defun gui-runner--pw-client-p (name)
  "Non-nil when pipewire lists a client whose description holds NAME, in any case.
WirePlumber 0.5 names its clients \"WirePlumber\" and lists no binary."
  (eq 0 (call-process "sh" nil nil nil "-c" (format "timeout 5 pw-cli ls Client | grep -qi %s" name))))

(defun gui-runner--cast-start ()
  "Start pipewire, wireplumber and a ScreenCast session on the headless monitor (once per run)."
  (unless gui-runner--cast-session
    (setq gui-runner--pw-procs (list (start-process "pipewire" " *pipewire*" "pipewire")))
    (unless (gui-runner-wait (lambda () (file-exists-p (expand-file-name "pipewire-0" (getenv "XDG_RUNTIME_DIR")))))
      (error "gui-runner: pipewire has no socket"))
    (push (start-process "wireplumber" " *wireplumber*" "wireplumber") gui-runner--pw-procs)
    (unless (gui-runner-wait (lambda () (gui-runner--pw-client-p "wireplumber")))
      (error "gui-runner: wireplumber did not connect"))
    (let* ((svc "org.gnome.Mutter.ScreenCast")
           (sess (dbus-call-method :session svc "/org/gnome/Mutter/ScreenCast" svc
                                   "CreateSession" :timeout 10000 '(:array :signature "{sv}"))))
      (glitnir-gui--say "gui-runner: cast session %s" sess)
      (dbus-call-method :session svc sess "org.gnome.Mutter.ScreenCast.Session" "RecordMonitor"
                        :timeout 10000 "" '(:array (:dict-entry "cursor-mode" (:variant :uint32 0))))
      (dbus-call-method :session svc sess "org.gnome.Mutter.ScreenCast.Session" "Start" :timeout 10000)
      (glitnir-gui--say "gui-runner: cast started")
      (setq gui-runner--cast-session sess)
      (unless (gui-runner-wait (lambda () (not (string-empty-p (gui-runner--cast-node)))))
        (error "gui-runner: mutter's stream has no pipewire node")))))

(defun gui-runner--cast-node ()
  "The pipewire node id of mutter's stream, as a string."
  (with-temp-buffer
    (call-process "sh" nil t nil "-c"
                  "timeout 10 pw-cli ls Node | awk '/^\\tid /{id=$2} /node.name = \"mutter\"/{print id}' | tr -d , | tail -1")
    (string-trim (buffer-string))))

(defun gui-runner--mutter-grab ()
  "Grab the screen through the ScreenCast session of `gui-runner--cast-start' and one `gst-launch-1.0' run."
  (gui-runner--cast-start)
  (let ((file (make-temp-file "gui-runner-grab-")) w h)
    (unwind-protect
        (progn
          (with-temp-buffer
            (let ((rc (call-process "timeout" nil t nil "15" "gst-launch-1.0" "-v" "pipewiresrc"
                                    (concat "path=" (gui-runner--cast-node)) "num-buffers=1" "!"
                                    "videoconvert" "!" "video/x-raw,format=BGRA" "!" "filesink"
                                    (concat "location=" file))))
              (unless (eq rc 0) (error "gui-runner-screen-grab: gst-launch-1.0 exited %S: %s" rc (buffer-string))))
            (goto-char (point-min))
            ;; the caps gst prints for the filesink's pad: "... width=(int)2880, height=(int)1800 ..."
            (unless (re-search-forward "filesink.*caps = .*?width=(int)\\([0-9]+\\), height=(int)\\([0-9]+\\)" nil t)
              (error "gui-runner-screen-grab: no size in gst's caps: %s" (buffer-string)))
            (setq w (string-to-number (match-string 1)) h (string-to-number (match-string 2))))
          (let ((bytes (gui-runner--read-bytes file)))
            (unless (>= (length bytes) (* 4 w h))
              (error "gui-runner-screen-grab: %d bytes for %dx%d" (length bytes) w h))
            (list w h (substring bytes 0 (* 4 w h)))))
      (delete-file file))))

(defun gui-runner--read-bytes (file)
  "The contents of FILE as a unibyte string."
  (with-temp-buffer
    (set-buffer-multibyte nil)
    (insert-file-contents-literally file)
    (buffer-string)))

(defun gui-runner--ppm-to-bgra (ppm)
  "(W H BYTES) from the binary PPM (P6) string PPM: RGB expanded to BGRA with alpha 255."
  (unless (string-match "\\`P6\\s-+\\([0-9]+\\)\\s-+\\([0-9]+\\)\\s-+255\\s-" ppm)
    (error "gui-runner-screen-grab: not a P6 maxval-255 PPM"))
  (let* ((w (string-to-number (match-string 1 ppm))) (h (string-to-number (match-string 2 ppm)))
         (start (match-end 0)) (n (* w h))
         (out (make-string (* 4 n) 0 nil)))
    (unless (>= (- (length ppm) start) (* 3 n))
      (error "gui-runner-screen-grab: PPM holds %d bytes for %dx%d" (- (length ppm) start) w h))
    (let ((i 0) (p start) (o 0))
      (while (< i n)
        (aset out o (aref ppm (+ p 2)))
        (aset out (+ o 1) (aref ppm (+ p 1)))
        (aset out (+ o 2) (aref ppm p))
        (aset out (+ o 3) 255)
        (setq i (1+ i) p (+ p 3) o (+ o 4))))
    (list w h out)))
(byte-compile 'gui-runner--ppm-to-bgra)

(defun gui-runner--sway-grab ()
  "Grab the headless output with grim (sway and labwc)."
  (let ((file (make-temp-file "gui-runner-grim-" nil ".ppm")))
    (unwind-protect
        (with-temp-buffer
          (let ((rc (call-process "grim" nil t nil "-t" "ppm" "-o" "HEADLESS-1" file)))
            (unless (eq rc 0) (error "gui-runner-screen-grab: grim exited %S: %s" rc (buffer-string))))
          (gui-runner--ppm-to-bgra (gui-runner--read-bytes file)))
      (delete-file file))))

(defun gui-runner--recorder-grab ()
  "Ask the recorder for its composite as bgra."
  (let* ((file (make-temp-file "gui-runner-rec-" nil ".bgra"))
         (reply (condition-case err
                    (gui-runner-recorder (list :cmd "snapshot" :path file :format "bgra"))
                  (error (delete-file file) (signal (car err) (cdr err))))))
    (unwind-protect
        (let ((w (plist-get reply :w)) (h (plist-get reply :h)) (bytes (gui-runner--read-bytes file)))
          (unless (= (length bytes) (* 4 w h))
            (error "gui-runner-screen-grab: recorder wrote %d bytes for %dx%d" (length bytes) w h))
          (list w h bytes))
      (delete-file file))))

(defun gui-runner--stable-grab (grab)
  "Call GRAB until two grabs in a row show the same picture (five at most), and return the last.
A commit and a grab reach the compositor on different connections, and it reports no frame number to the grabber: the
first grab can be the frame before the one just committed."
  (let ((pic (funcall grab)) (n 0) next)
    (while (and (< n 4) (not (equal pic (setq next (funcall grab)))))
      (setq pic next n (1+ n)))
    pic))

(defun gui-runner-screen-grab ()
  "Grab the compositor's output: a list (W H BYTES), BYTES a unibyte string of W*H BGRA pixels (Cairo ARGB32,
little-endian), the shape of `glitnir-snapshot's `raw'.  Sway and labwc: grim; KWin: none (the row skips); recorder: its `snapshot'; mutter: ScreenCast."
  (pcase (glitnir-gui--compositor)
    ((or "sway" "labwc") (gui-runner--stable-grab #'gui-runner--sway-grab))
    ("kwin" (ert-skip "no headless grab on KWin"))
    ("recorder" (gui-runner--recorder-grab))
    ("mutter" (gui-runner--stable-grab #'gui-runner--mutter-grab))
    (c (error "gui-runner-screen-grab: unknown compositor %S" c))))

(defun gui-runner--sway-node-shown-p (node)
  "Non-nil when NODE of `swaymsg -t get_tree' (an alist) or a node below it is a visible window of Nidhogg."
  (or (and (equal (alist-get 'app_id node) "nidhogg") (alist-get 'visible node))
      (seq-some #'gui-runner--sway-node-shown-p
                (append (alist-get 'nodes node) (alist-get 'floating_nodes node)))))

(defun gui-runner--sway-window-shown-p ()
  "Non-nil when `swaymsg -t get_tree' shows a visible window whose app_id is Nidhogg's (not just any visible node:
the workspaces and the output are \"visible\" before any window maps)."
  (with-temp-buffer
    (and (eq 0 (call-process "swaymsg" nil t nil "-t" "get_tree"))
         (progn (goto-char (point-min))
                (gui-runner--sway-node-shown-p
                 (json-parse-string (buffer-string) :object-type 'alist :array-type 'list
                                    :false-object nil :null-object nil))))))

(defun gui-runner--presenter-counts ()
  "(PUBLISHED PRESENTED SENT) of the selected frame's display: the newest request's number, the newest that has gone
out with every older one, and the commits sent (`nhwl-presenter-stats').  Nil on a frame with no presenter, or when
the display is gone."
  (when-let* (((fboundp 'nhwl-presenter-stats))
              (s (ignore-errors (nhwl-presenter-stats))))
    (list (plist-get s :published) (plist-get s :presented)
          (+ (plist-get s :commits) (plist-get s :bare-commits)))))

(defun gui-runner--presented-p (&optional now)
  "Non-nil when every request of the frame's display has gone out to the compositor in a flushed commit.
NOW is the `gui-runner--presenter-counts' to judge (nil: read them now).  Each picture published, configure ack and
piece of surface state takes a number, and the presenter's `:presented' passes one only once its commit, and every
older one's, is flushed: no other commit (a bare ack, a child frame's, the one before) stands for it."
  (let ((now (or now (gui-runner--presenter-counts))))
    (or (null now) (>= (nth 1 now) (nth 0 now)))))

(defun gui-runner--recorder-next ()
  "The recorder's next commit sequence number (a :since past the end, so no commit is copied)."
  (plist-get (gui-runner-recorder '(:cmd "log" :since 1000000000)) :next))

(defun gui-runner--recorder-quiet (seen)
  "A predicate: every request has gone out (`gui-runner--presented-p'), every frame callback is answered, and the
recorder's commit log stands still between two calls.  SEEN is a cons whose car keeps the last sequence number.  The
presenter's counters are read before the recorder is asked: a commit counts once flushed, and one flushed before the
first of the two replies has been read by the second, so the log read here holds every commit counted before.  They
are read again after: a request numbered or a commit sent meanwhile (a query's redisplay can publish) is for the next
call.  While the output is occluded the callbacks wait: only the log counts."
  (lambda ()
    (let* ((counts (gui-runner--presenter-counts))
           (o (gui-runner-recorder '(:cmd "outstanding")))
           (occluded (eq (plist-get o :occluded) t))
           (next (gui-runner--recorder-next))
           (still (eql next (car seen))))
      (setcar seen next)
      (and still
           (or occluded
               (and (eql 0 (plist-get o :callbacks)) (gui-runner--presented-p counts)
                    (equal counts (gui-runner--presenter-counts))))))))

(defun gui-runner-wait-callbacks-answered ()
  "Wait (5 s at most) for the recorder to have answered every frame callback, with Emacs reading no input meanwhile:
the answers and whatever the recorder sent before them are in the socket for one read."
  (let ((deadline (+ (float-time) 5)))
    (while (and (not (eql 0 (plist-get (gui-runner-recorder '(:cmd "outstanding")) :callbacks)))
                (< (float-time) deadline))
      (accept-process-output nil 0.01))))

(defun gui-runner-wait-settled (&optional read)
  "Wait until what the frame last drew has reached the compositor, and fail the running row if that takes over 3 s.
With READ, read and drop the events meanwhile: a commit can wait for a configure Emacs has not read yet.
The frame's side, on the native shell: every request has gone out (`gui-runner--presented-p'); on pgtk: a paint after
it (`glitnir-snapshot' asks for one and returns after GDK's after-paint, which commits).  The compositor's side,
recorder: the frame callbacks are answered and the commit log stands still (a dead control socket is an error); sway:
`swaymsg -t get_tree' shows a window of Nidhogg.  A later grab is a later request on the compositor's event loop."
  (redisplay t)
  (let* ((compositor (glitnir-gui--compositor)) (native (featurep 'nidhogg-wayland))
         (graphic (display-graphic-p)) (ok t)
         (settle (lambda (pred)
                   (let ((deadline (+ (float-time) 3)) (v nil))
                     (while (and (not (setq v (funcall pred))) (< (float-time) deadline))
                       (if read (gui-runner-read-all) (sit-for 0.01)))
                     (unless v (setq ok nil))))))
    (when (and graphic (not native) (not (featurep 'ns)) (fboundp 'glitnir-snapshot))
      (ignore-errors (glitnir-snapshot nil 'raw)))
    (pcase compositor
      ("sway" (funcall settle (lambda () (and (gui-runner--sway-window-shown-p) (gui-runner--presented-p)))))
      ("recorder" (funcall settle (gui-runner--recorder-quiet (list nil))))
      (_ (funcall settle #'gui-runner--presented-p)))
    (unless ok
      (let ((why (format "gui-runner: not settled in 3 s: presenter %S, selected %S, frames %S%s"
                         (gui-runner--presenter-counts)
                         (selected-frame) (mapcar (lambda (f) (cons f (frame-visible-p f))) (frame-list))
                         (if (equal compositor "recorder")
                             (format ", outstanding %S" (gui-runner-recorder '(:cmd "outstanding")))
                           ""))))
        (glitnir-gui--say "%s" why)
        (when (ert-running-test) (ert-fail why))))))

(defun gui-runner-pixel-diff (a b &optional rect)
  "Compare two grabs A and B (each (W H BYTES)) over RECT, a list (X Y W H) in both (default: the common top-left
area).  Returns (MAX-CHANNEL-DIFF . PIXELS-OVER-2): the largest per-channel difference and the number of pixels
with any channel differing by more than 2."
  (pcase-let* ((`(,aw ,ah ,ab) a) (`(,bw ,bh ,bb) b)
               (`(,rx ,ry ,rw ,rh) (or rect (list 0 0 (min aw bw) (min ah bh)))))
    (when (or (> (+ rx rw) (min aw bw)) (> (+ ry rh) (min ah bh)))
      (error "gui-runner-pixel-diff: rect %S outside the grabs (%dx%d, %dx%d)" (list rx ry rw rh) aw ah bw bh))
    (gui-runner--pixel-diff aw ab bw bb rx ry rw rh)))

(defun gui-runner--pixel-diff (aw ab bw bb rx ry rw rh)
  (let ((worst 0) (over 0) (y ry) (ye (+ ry rh)) (xe (+ rx rw)))
    (while (< y ye)
      (let ((x rx))
        (while (< x xe)
          (let ((i (* 4 (+ x (* y aw)))) (j (* 4 (+ x (* y bw)))) (px 0) (c 0))
            (while (< c 4)
              (let ((d (abs (- (aref ab (+ i c)) (aref bb (+ j c))))))
                (when (> d px) (setq px d)))
              (setq c (1+ c)))
            (when (> px worst) (setq worst px))
            (when (> px 2) (setq over (1+ over))))
          (setq x (1+ x))))
      (setq y (1+ y)))
    (cons worst over)))
(byte-compile 'gui-runner--pixel-diff)

;;; The fake portal (gui.sh --portal fake; see fake-portal.el)

(defun gui-runner-portal-script (replies)
  "Replace the fake portal's script with REPLIES, a list of plists, one per coming call.
Each is (:response N [:uris (URI...)]) or (:never t)."
  (let ((file (or (getenv "NHWL_FAKE_PORTAL_SCRIPT") (error "gui-runner: the run has no fake portal (--portal fake)"))))
    (with-temp-file file
      (dolist (r replies) (insert (prin1-to-string r) "\n")))))

(defun gui-runner-portal-log ()
  "The fake portal's log so far: a list of (:method ... ) and (:close PATH) plists, oldest first."
  (let ((file (or (getenv "NHWL_FAKE_PORTAL_LOG") (error "gui-runner: the run has no fake portal (--portal fake)"))))
    (with-temp-buffer
      (insert-file-contents file)
      (mapcar #'read (split-string (buffer-string) "\n" t)))))

;;; Restarting the session's compositor (gui.sh sets NHWL_GUI_RESTART on the recorder and on sway only)

(defun gui-runner-restart-available-p ()
  "Non-nil when the run's compositor can be stopped and started again on the same socket."
  (let ((script (getenv "NHWL_GUI_RESTART")))
    (and script (not (string-empty-p script)) (file-executable-p script))))

(defun gui-runner-restart-note ()
  "Tell the trace that the compositor is about to stop on purpose, so the lost connection is no `errors' violation.
The text is `wl-trace-check-restart-line'."
  (glitnir-gui--say "gui-runner: restarting the compositor"))

(defun gui-runner-restart-compositor ()
  "Stop the compositor and start it again on the same socket; returns when it serves (the script printed READY).
The recorder keeps its session store.  Signals an error when the script fails or the run has none."
  (unless (gui-runner-restart-available-p)
    (error "gui-runner: this compositor has no restart script (NHWL_GUI_RESTART)"))
  (gui-runner-restart-note)
  (with-temp-buffer
    (let ((rc (call-process (getenv "NHWL_GUI_RESTART") nil t nil)))
      (unless (and (eq rc 0) (string-match-p "^READY$" (buffer-string)))
        (error "gui-runner: the restart script failed (%S): %s" rc (buffer-string))))))

(defun glitnir-gui--load-tests ()
  (let ((dir (or (getenv "GLITNIR_GUI_TESTS") glitnir-gui--dir)))
    (dolist (f (let ((glob (getenv "GLITNIR_GUI_FILES")))
                 (if (and glob (not (string-empty-p glob)))
                     ;; each glob in turn, its files sorted: the order the rows run in
                     (mapcan (lambda (g) (sort (file-expand-wildcards (expand-file-name g dir) t) #'string<))
                             (split-string glob))
                   (seq-remove (lambda (f)
                                 (and (not (featurep 'ns)) (string-match-p "\\(-ns\\|nhmac\\)-gui-tests\\.el\\'" f)))
                               (directory-files dir t "-gui\\(-[a-z0-9]+\\)?-tests\\.el\\'")))))
      (load f nil t))))

;; The known-differences file holds entries (TEST-NAME COMPOSITOR REASON . OPTIONS): the test is expected to fail on that
;; compositor, for REASON (a string).  OPTIONS is a plist: `:either t' also accepts a pass (a row that lands on both
;; sides there).  A failure is a failed assertion: an error
;; in a listed row is a different failure and is unexpected.  A skipped row is expected.
(defun gui-runner-known-differences ()
  "The entries of the file GLITNIR_GUI_KNOWN that apply to this compositor."
  (let ((file (getenv "GLITNIR_GUI_KNOWN")))
    (when (and file (file-readable-p file))
      (with-temp-buffer
        (insert-file-contents file)
        (seq-filter (lambda (e)
                      (equal (nth 1 e) (glitnir-gui--compositor)))
                    (read (current-buffer)))))))

(defun glitnir-gui--assertion-failure-p (result)
  "Non-nil when RESULT is a failed `should' (not an error signalled by the row)."
  (and (ert-test-failed-p result)
       (eq (car-safe (ert-test-result-with-condition-condition result)) 'ert-test-failed)))

(defun glitnir-gui--apply-known-differences ()
  "Make the known differences the rows' expected results (before `ert-run-tests')."
  (dolist (entry (gui-runner-known-differences))
    (let ((name (let ((n (nth 0 entry))) (if (symbolp n) n (intern n)))))
      (when (ert-test-boundp name)
        (setf (ert-test-expected-result-type (ert-get-test name))
              (if (plist-get (nthcdr 3 entry) :either)
                  '(or :passed :skipped (satisfies glitnir-gui--assertion-failure-p))
                '(or :skipped (satisfies glitnir-gui--assertion-failure-p))))))))

(defun glitnir-gui--frame-drew-p ()
  "Non-nil when the selected frame is visible and has drawn (Naglfar's `:draws' at least 1); t off Naglfar."
  (or (not (fboundp 'nhmac-stats))
      (progn (redisplay t)
             (and (frame-visible-p (selected-frame))
                  (>= (or (plist-get (funcall 'nhmac-stats) :draws) 0) 1)))))

(defun glitnir-gui--fail-drawing-rows (tests)
  "When the frame never drew, make TESTS tagged `draws' fail at once with the reason, not wait out their timeouts.
The rows without the tag (locks, fds, threads) still run."
  (unless (glitnir-gui--frame-drew-p)
    (let ((why (format "no frame drew at scale %s" (glitnir-gui--scale))))
      (glitnir-gui--say "gui-runner: %s: the rows tagged `draws' fail without running" why)
      (glitnir-gui--say "gui-runner: the shell's view: %S"
                        (condition-case err (funcall 'nhmac--shell-view) (error (list 'error err))))
      (dolist (test tests)
        (when (memq 'draws (ert-test-tags test))
          (setf (ert-test-body test) (lambda () (ert-fail (list why)))))))))

(defvar glitnir-gui--row-start 0 "When the running row started (`float-time').")

(defun glitnir-gui--report (event-type &rest args)
  (pcase event-type
    ;; a row's start, so a row that ends Emacs is named by the lane (nhmac-gui.sh) as its one red row
    ('test-started
     (setq glitnir-gui--row-start (float-time))
     (glitnir-gui--say "started %s" (ert-test-name (nth 1 args))))
    ('test-ended
     (let* ((test (nth 1 args)) (result (nth 2 args))
            (expected (ert-test-result-expected-p test result)))
       ;; with the row's seconds, so a slow lane names its slow rows
       (glitnir-gui--say "%s %s (%.1f s)"
                         (cond ((not expected) "FAILED") ((ert-test-skipped-p result) "skipped") (t "passed"))
                         (ert-test-name test) (- (float-time) glitnir-gui--row-start))
       (unless expected
         (when (ert-test-result-with-condition-p result)
           (glitnir-gui--say "  %S" (ert-test-result-with-condition-condition result))
           ;; the `ert-info' lines, innermost first
           (dolist (info (ert-test-result-with-condition-infos result))
             (glitnir-gui--say "  %s%s" (car info) (cdr info)))))
       ;; logged: the result keeps no row's values (a capture's pixels in a `should' form, a backtrace's
       ;; arguments), which would stay live to the end and make every later row's GC mark them
       (setf (ert-test-result-should-forms result) nil)
       (when (ert-test-result-with-condition-p result)
         (setf (ert-test-result-with-condition-condition result) nil
               (ert-test-result-with-condition-backtrace result) nil
               (ert-test-result-with-condition-infos result) nil))))))

(defun glitnir-gui--selector ()
  "The ERT selector: GLITNIR_GUI_SELECT (an Emacs regexp, default every test), and with GLITNIR_GUI_TAG set
\(and REGEXP (tag TAG))."
  (let ((re (let ((r (getenv "GLITNIR_GUI_SELECT"))) (and r (not (string-empty-p r)) r)))
        (tag (let ((x (getenv "GLITNIR_GUI_TAG"))) (and x (not (string-empty-p x)) (intern x)))))
    (let ((sel (cond ((and re tag) `(and ,re (tag ,tag)))
                     (tag `(tag ,tag))
                     (t (or re t))))
          (only (let ((o (getenv "GLITNIR_GUI_ONLY"))) (and o (not (string-empty-p o)) (intern o)))))
      ;; GLITNIR_GUI_ONLY: that one row, when the selector takes it (a lane's fresh session)
      (if only `(and ,sel (member ,only)) sel))))

(defun glitnir-gui--done ()
  "The rows GLITNIR_GUI_DONE names: reported by an earlier session of this run."
  (let ((file (getenv "GLITNIR_GUI_DONE")))
    (when (and file (file-readable-p file))
      (with-temp-buffer
        (insert-file-contents file)
        (mapcar #'intern (split-string (buffer-string) "\n" t "[ \t]+"))))))

(defun glitnir-gui--ordered (tests)
  "TESTS in the lane's fixed order: by the file that defines each (the load order), then by name."
  (let ((files (mapcar #'car load-history)))
    (sort (copy-sequence tests)
          (lambda (a b)
            (let ((fa (or (seq-position files (ert-test-file-name a)) 0))
                  (fb (or (seq-position files (ert-test-file-name b)) 0)))
              ;; `load-history' is newest first: a later-loaded file has the smaller position
              (if (/= fa fb) (> fa fb)
                (string< (symbol-name (ert-test-name a)) (symbol-name (ert-test-name b)))))))))

(defun glitnir-gui--run ()
  (glitnir-gui--place-frame)
  (glitnir-gui--say-frame "placed")
  (glitnir-gui--wait-for-scale)
  (glitnir-gui--wait-for-first-paint)
  (glitnir-gui--say-frame "first paint")
  (glitnir-gui--say-startup)
  ;; the heap before any row, against which a timing row's heap is read
  (glitnir-gui--say "gui-runner: heap at start %S" (garbage-collect))
  (glitnir-gui--load-tests)
  (glitnir-gui--apply-known-differences)
  (let* ((done (glitnir-gui--done))
         (tests (glitnir-gui--ordered
                 (seq-remove (lambda (test) (memq (ert-test-name test) done))
                             (ert-select-tests (glitnir-gui--selector) t))))
         (_ (glitnir-gui--fail-drawing-rows tests))
         ;; rows tagged `runs-last' (one that ends the compositor, say) run after every other row
         (last (seq-filter (lambda (test) (memq 'runs-last (ert-test-tags test))) tests))
         (stats (ert-run-tests `(member ,@(seq-difference tests last) ,@last) #'glitnir-gui--report))
         (n (ert-stats-total stats)) (bad (ert-stats-completed-unexpected stats))
         (skipped (ert-stats-skipped stats))
         (parity (seq-filter (lambda (test) (memq 'parity (ert-test-tags test))) tests))
         (parity-bad (seq-count (lambda (test)
                                  (let ((r (ert-test-most-recent-result test)))
                                    (and r (not (ert-test-result-expected-p test r)))))
                                parity)))
    (glitnir-gui--say "Ran %d tests, %d as expected (%d skipped), %d unexpected" n (- n bad) skipped bad)
    (when parity
      (glitnir-gui--say "gui-runner: parity rows %d, %d unexpected; functional rows %d, %d unexpected"
                        (length parity) parity-bad (- n (length parity)) (- bad parity-bad)))
    ;; a fresh session's one row that the selector leaves out ran nothing, as asked
    (kill-emacs (if (and (or (> n 0) (not (member (getenv "GLITNIR_GUI_ONLY") '(nil "")))) (= bad 0)) 0 1))))

(defun glitnir-gui--start (&rest _)
  "Run the tests once, after the first frame is visible and redisplayed."
  (remove-hook 'server-after-make-frame-hook #'glitnir-gui--start)
  (remove-hook 'window-setup-hook #'glitnir-gui--start)
  (run-at-time 0 nil
               (lambda ()
                 (condition-case err
                     (progn (redisplay t) (glitnir-gui--run))
                   (error (glitnir-gui--say "gui-runner: %S" err) (kill-emacs 1))))))

;; `window-setup-hook' runs at the end of `command-line'; the timer fires in the command loop after it.
(if (daemonp)
    (add-hook 'server-after-make-frame-hook #'glitnir-gui--start)
  (add-hook 'window-setup-hook #'glitnir-gui--start))

;;; gui-runner.el ends here
