;;; pgtk-gui-tests.el --- GUI tests for Nidhogg's pgtk frame handling  -*- lexical-binding: t -*-

;; Run by nidhogg/test/gui.sh on a headless mutter (gui-runner.el loads every *-gui-tests.el).

(require 'ert)
(require 'benchmark)

(ert-deftest nidhogg-gui/hide-returns-before-the-timeout ()
  (let ((child (make-frame `((parent-frame . ,(selected-frame)) (width . 20) (height . 5)
                             (visibility . t)))))
    (redisplay t)
    (sit-for 0.2)
    ;; The timeout raised far above any scheduling noise: a hide that waits it out takes 2 s, one that returns once
    ;; the pending events are handled takes milliseconds (a fixed 30 ms bound failed at scale 2 on CI: 36 ms).
    (let ((elapsed (let ((pgtk-wait-for-event-timeout 2.0))
                     (benchmark-elapse (make-frame-invisible child)))))
      (princ (format "hide-time: %.4f s\n" elapsed) #'external-debugging-output)
      (should (< elapsed 1.0)))
    (should-not (frame-visible-p child))
    (delete-frame child)))

(ert-deftest nidhogg-gui/show-then-hide-stays-hidden ()
  "The race the wait in `pgtk_make_frame_invisible' exists for."
  (let ((child (make-frame `((parent-frame . ,(selected-frame)) (width . 20) (height . 5)
                             (visibility . nil)))))
    (make-frame-visible child)
    (make-frame-invisible child)
    (sit-for 0.3)
    (should-not (frame-visible-p child))
    (delete-frame child)))

(ert-deftest nidhogg-gui/hiding-a-root-frame-still-works ()
  (let ((root (make-frame '((width . 40) (height . 10) (visibility . t)))))
    (redisplay t)
    (sit-for 0.2)
    (make-frame-invisible root)
    (sit-for 0.3)
    (should-not (frame-visible-p root))
    (delete-frame root)))

;;; pgtk-gui-tests.el ends here
