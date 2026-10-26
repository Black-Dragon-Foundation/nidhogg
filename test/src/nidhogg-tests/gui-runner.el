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
;;   GLITNIR_GUI_KNOWN       the known-differences file (gui-known-differences.eld next to the tests)
;; Prints every result to `external-debugging-output', then `kill-emacs' 0 (all as expected) or 1.
;; The tests start only once the first frame is visible and has been redisplayed, never inside `command-line'.

(require 'ert)

(defconst glitnir-gui--dir (file-name-directory (or load-file-name buffer-file-name)))

(defun glitnir-gui--say (fmt &rest args)
  (let ((s (apply #'format fmt args)))
    (princ (concat s "\n") #'external-debugging-output)))

(defun glitnir-gui--scale ()
  (string-to-number (or (getenv "GLITNIR_GUI_SCALE") "1")))

(defun glitnir-gui--compositor ()
  "The compositor gui.sh runs on: \"sway\", \"mutter\" or \"recorder\" (mutter when unset)."
  (or (getenv "GLITNIR_GUI_COMPOSITOR") "mutter"))

(defun glitnir-gui--scale-ready-p ()
  "Non-nil once the selected frame sits at the asked scale (the monitor's scale applied), by the rule of the compositor."
  (pcase (glitnir-gui--compositor)
    ;; Probed 2026-10-26 on the .2 prefix, sway, 2880x1800: scale 1 gives (frame-scale-factor) 1.0 and geometry
    ;; 2880x1800; 1.25 gives 2.0 and 4608x2880; 1.5 gives 2.0 and 3840x2400; 2 gives 2.0 and 2880x1800.  That is
    ;; mutter's rule too.  The recorder (profile sway) sends the same wl_output scale and xdg-output size.
    ((or "sway" "mutter" "recorder") (glitnir-gui--gtk3-scale-ready-p))
    (c (error "gui-runner: unknown compositor %S" c))))

(defun glitnir-gui--gtk3-scale-ready-p ()
  (let ((s (glitnir-gui--scale)))
    (if (featurep 'nidhogg-wayland)
        ;; Nidhogg's own Wayland shell has the exact scale and the monitor's device size
        (and (< (abs (- (frame-scale-factor) s)) 0.001)
             (let ((size (getenv "GLITNIR_GUI_SIZE"))
                   (geo (alist-get 'geometry (car (display-monitor-attributes-list)))))
               (or (not size) (equal (format "%dx%d" (nth 2 geo) (nth 3 geo)) size))))
      ;; GTK 3 knows only integer scales: it reports ceil(S) and a monitor geometry of the logical size (WxH / S)
      ;; times ceil(S) (S = 1.25 on 2880x1800: 4608x2880), which tells S = 1.25 from S = 2.
      (and (= (frame-scale-factor) (ceiling s))
           (let* ((size (getenv "GLITNIR_GUI_SIZE"))
                  (geo (alist-get 'geometry (car (display-monitor-attributes-list))))
                  (m (and size (string-match "\\`\\([0-9]+\\)x\\([0-9]+\\)\\'" size))))
             (or (not m)
                 (and (= (nth 2 geo) (* (ceiling s) (round (/ (string-to-number (match-string 1 size)) s))))
                      (= (nth 3 geo) (* (ceiling s) (round (/ (string-to-number (match-string 2 size)) s)))))))))))

(defun glitnir-gui--wait-for-scale ()
  (let ((deadline (+ (float-time) 10)))
    (while (and (not (glitnir-gui--scale-ready-p)) (< (float-time) deadline))
      (sit-for 0.1))
    (unless (glitnir-gui--scale-ready-p)
      (glitnir-gui--say "gui-runner: scale %s not reached in 10 s (frame-scale-factor %s, monitors %S)"
                        (glitnir-gui--scale) (frame-scale-factor) (display-monitor-attributes-list))
      (kill-emacs 1))))

(defun glitnir-gui--wait-for-first-paint ()
  "Wait (10 s at most) until Glitnir's frame-time record holds a paint of the selected frame."
  (when (fboundp 'glitnir-stats)
    (let ((deadline (+ (float-time) 10)))
      (while (and (< (or (plist-get (glitnir-stats) :count) 0) 1) (< (float-time) deadline))
        (redisplay t)
        (sit-for 0.1)))))

;;; Grabs from the compositor: (gui-runner-screen-grab) is (W H BYTES), BYTES W*H BGRA pixels, on every compositor

(require 'cl-lib)
(require 'dbus)
(require 'json)
(require 'seq)

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

(defvar gui-runner--pw-procs nil "The pipewire and wireplumber processes this session started.")
(defvar gui-runner--cast-session nil "The ScreenCast session's D-Bus path, held by Emacs's own connection.")

(defun gui-runner--cast-start ()
  "Start pipewire, wireplumber and a ScreenCast session on the headless monitor (once per run)."
  (unless gui-runner--cast-session
    (setq gui-runner--pw-procs
          (list (start-process "pipewire" " *pipewire*" "pipewire")
                (progn (sleep-for 1) (start-process "wireplumber" " *wireplumber*" "wireplumber"))))
    (sleep-for 1)
    (let* ((svc "org.gnome.Mutter.ScreenCast")
           (sess (dbus-call-method :session svc "/org/gnome/Mutter/ScreenCast" svc
                                   "CreateSession" :timeout 10000 '(:array :signature "{sv}"))))
      (glitnir-gui--say "gui-runner: cast session %s" sess)
      (dbus-call-method :session svc sess "org.gnome.Mutter.ScreenCast.Session" "RecordMonitor"
                        :timeout 10000 "" '(:array (:dict-entry "cursor-mode" (:variant :uint32 0))))
      (dbus-call-method :session svc sess "org.gnome.Mutter.ScreenCast.Session" "Start" :timeout 10000)
      (glitnir-gui--say "gui-runner: cast started")
      (setq gui-runner--cast-session sess)
      (sleep-for 1.5))))

(defun gui-runner--cast-node ()
  "The pipewire node id of mutter's stream, as a string."
  (with-temp-buffer
    (call-process "sh" nil t nil "-c"
                  "timeout 10 pw-cli ls Node | awk '/^\\tid /{id=$2} /node.name = \"mutter\"/{print id}' | tr -d , | tail -1")
    (string-trim (buffer-string))))

(defun gui-runner--mutter-grab ()
  "Grab the screen through the ScreenCast session of `gui-runner--cast-start' and one `gst-launch-1.0' run."
  (gui-runner--cast-start)
  (sit-for 0.3)
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
  "Grab the headless output with grim."
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

(defun gui-runner-screen-grab ()
  "Grab the compositor's output: a list (W H BYTES), BYTES a unibyte string of W*H BGRA pixels (Cairo ARGB32,
little-endian), the shape of `glitnir-snapshot's `raw'.  Sway: grim; recorder: its `snapshot'; mutter: ScreenCast."
  (pcase (glitnir-gui--compositor)
    ("sway" (gui-runner--sway-grab))
    ("recorder" (gui-runner--recorder-grab))
    ("mutter" (gui-runner--mutter-grab))
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

(defun gui-runner--recorder-commit-after-p (since)
  "Non-nil when the recorder logged a toplevel commit with a seq of at least SINCE."
  (seq-some (lambda (c) (equal (plist-get c :role) "toplevel"))
            (plist-get (gui-runner-recorder (list :cmd "log" :since since)) :commits)))

(defun gui-runner-wait-settled ()
  "Wait until the compositor shows what the frame last drew.
Sway: two frame periods (60 Hz) after `swaymsg -t get_tree' shows a window of Nidhogg.  Recorder: a toplevel commit
newer than this call (0.5 s at most: a redisplay that changes nothing commits nothing); a dead control socket is an
error.  Mutter: redisplay and 0.5 s."
  (let ((since (and (equal (glitnir-gui--compositor) "recorder")
                    (plist-get (gui-runner-recorder '(:cmd "log" :since 0)) :next))))
    (redisplay t)
    (pcase (glitnir-gui--compositor)
      ("sway"
       (let ((deadline (+ (float-time) 10)))
         (while (and (not (gui-runner--sway-window-shown-p)) (< (float-time) deadline))
           (sit-for 0.1)))
       (sit-for (/ 2.0 60)))
      ("recorder"
       (let ((deadline (+ (float-time) 0.5)))
         (while (and (not (gui-runner--recorder-commit-after-p since)) (< (float-time) deadline))
           (sit-for 0.02))))
      (_ (sit-for 0.5)))))

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

(defun glitnir-gui--load-tests ()
  (let ((dir (or (getenv "GLITNIR_GUI_TESTS") glitnir-gui--dir)))
    (dolist (f (let ((glob (getenv "GLITNIR_GUI_FILES")))
                 (if (and glob (not (string-empty-p glob)))
                     (file-expand-wildcards (expand-file-name glob dir) t)
                   (directory-files dir t "-gui-tests\\.el\\'"))))
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

(defun glitnir-gui--report (event-type &rest args)
  (pcase event-type
    ('test-ended
     (let* ((test (nth 1 args)) (result (nth 2 args))
            (expected (ert-test-result-expected-p test result)))
       (glitnir-gui--say "%s %s"
                         (cond ((not expected) "FAILED") ((ert-test-skipped-p result) "skipped") (t "passed"))
                         (ert-test-name test))
       (unless expected
         (when (ert-test-result-with-condition-p result)
           (glitnir-gui--say "  %S" (ert-test-result-with-condition-condition result))))))))

(defun glitnir-gui--selector ()
  "The ERT selector: GLITNIR_GUI_SELECT (an Emacs regexp, default every test), and with GLITNIR_GUI_TAG set
\(and REGEXP (tag TAG))."
  (let ((re (let ((r (getenv "GLITNIR_GUI_SELECT"))) (and r (not (string-empty-p r)) r)))
        (tag (let ((x (getenv "GLITNIR_GUI_TAG"))) (and x (not (string-empty-p x)) (intern x)))))
    (cond ((and re tag) `(and ,re (tag ,tag)))
          (tag `(tag ,tag))
          (t (or re t)))))

(defun glitnir-gui--run ()
  (glitnir-gui--wait-for-scale)
  (glitnir-gui--wait-for-first-paint)
  (glitnir-gui--load-tests)
  (glitnir-gui--apply-known-differences)
  (let* ((stats (ert-run-tests (glitnir-gui--selector) #'glitnir-gui--report))
         (n (ert-stats-total stats)) (bad (ert-stats-completed-unexpected stats))
         (skipped (ert-stats-skipped stats)))
    (glitnir-gui--say "Ran %d tests, %d as expected (%d skipped), %d unexpected" n (- n bad) skipped bad)
    (kill-emacs (if (and (> n 0) (= bad 0)) 0 1))))

(defun glitnir-gui--start (&rest _)
  "Run the tests once, after the first frame is visible and redisplayed."
  (remove-hook 'server-after-make-frame-hook #'glitnir-gui--start)
  (remove-hook 'window-setup-hook #'glitnir-gui--start)
  (run-at-time 0 nil
               (lambda ()
                 (condition-case err
                     (progn (redisplay t) (sit-for 0.2) (glitnir-gui--run))
                   (error (glitnir-gui--say "gui-runner: %S" err) (kill-emacs 1))))))

;; `window-setup-hook' runs at the end of `command-line'; the timer fires in the command loop after it.
(if (daemonp)
    (add-hook 'server-after-make-frame-hook #'glitnir-gui--start)
  (add-hook 'window-setup-hook #'glitnir-gui--start))

;;; gui-runner.el ends here
