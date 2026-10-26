;;; recorder-gui-tests.el --- GUI tests against nh-recorder  -*- lexical-binding: t -*-

;; Run by nidhogg/test/gui.sh `--compositor recorder' (Task 4 teaches gui.sh), where $GLITNIR_GUI_RECORDER is the
;; recorder's control socket.  Task 5 moves `gui-runner-recorder' into gui-runner.el.  The rows skip without it.

(require 'ert)
(require 'json)

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

(defun recorder-gui--commits ()
  (plist-get (gui-runner-recorder '(:cmd "log" :since 0)) :commits))

(ert-deftest recorder-gui/pgtk-frame-maps-as-toplevel ()
  (skip-unless (getenv "GLITNIR_GUI_RECORDER"))
  (let* ((scale (frame-scale-factor))
         (w (* scale (frame-pixel-width)))
         (h (* scale (frame-pixel-height)))
         (hit (seq-find (lambda (c)
                          (and (equal (plist-get c :role) "toplevel")
                               (equal (plist-get c :app_id) "nidhogg")
                               (plist-get c :buffer)
                               (= (plist-get (plist-get c :buffer) :w) w)
                               (= (plist-get (plist-get c :buffer) :h) h)))
                        (recorder-gui--commits))))
    (should hit)))

(ert-deftest recorder-gui/pgtk-acks-maximize ()
  (skip-unless (getenv "GLITNIR_GUI_RECORDER"))
  (let* ((seq-before (plist-get (gui-runner-recorder '(:cmd "log" :since 0)) :next))
         (serial (plist-get (gui-runner-recorder
                             '(:cmd "configure" :surface "focused" :size [800 600] :states ["maximized" "activated"]))
                            :serial))
         (acked nil)
         (deadline (+ (float-time) 2)))
    (while (and (not acked) (< (float-time) deadline))
      (accept-process-output nil 0.05)
      (setq acked (seq-find (lambda (c)
                              (and (>= (plist-get c :seq) seq-before)
                                   (eql (plist-get c :ack_serial) serial)))
                            (recorder-gui--commits))))
    (should acked)
    (should-not (eq (plist-get acked :ack_stale) t))
    (should (eq (frame-parameter nil 'fullscreen) 'maximized))))

;;; recorder-gui-tests.el ends here
