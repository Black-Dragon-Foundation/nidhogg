;;; workers-image-gui-tests.el --- GUI tests for workers: image  -*- lexical-binding: t -*-

;; Copyright (C) 2026 Black Dragon Foundation contributors

;; This file is part of Nidhogg.

;; Nidhogg is free software: you can redistribute it and/or modify
;; it under the terms of the GNU General Public License as published by
;; the Free Software Foundation, either version 3 of the License, or
;; (at your option) any later version.

;; Nidhogg is distributed in the hope that it will be useful,
;; but WITHOUT ANY WARRANTY; without even the implied warranty of
;; MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
;; GNU General Public License for more details.

;; You should have received a copy of the GNU General Public License
;; along with Nidhogg.  If not, see <https://www.gnu.org/licenses/>.

;;; Code:

(require 'ert)
(require 'cl-lib)

;; Run by nidhogg/test/gui.sh (gui-runner.el loads every *-gui-tests.el).

(defvar workers-image-gui--dir
  (file-name-as-directory
   (or (getenv "NH_WORKERS_FIXTURES")
       (expand-file-name "fixtures/images/"
                         (file-name-directory
                          (or load-file-name buffer-file-name)))))
  "The image corpus (the runner stages the tests apart from it).")

(defvar workers-image-gui--serial 0)

(defun workers-image-gui--read (name)
  (with-temp-buffer
    (insert-file-contents (expand-file-name name workers-image-gui--dir))
    (goto-char (point-min))
    (read (current-buffer))))

(defun workers-image-gui--type (file)
  (pcase (file-name-extension file)
    ("png" 'png) ("jpg" 'jpeg) ("gif" 'gif) ("webp" 'webp)))

(defun workers-image-gui--spec (file bg index)
  `(image :type ,(workers-image-gui--type file)
          :file ,(expand-file-name file workers-image-gui--dir)
          ,@(and bg (list :background bg))
          ,@(and index (list :index index))))

(defun workers-image-gui--digest (r)
  (list (plist-get r :width) (plist-get r :height)
        (and (plist-get r :pixels) (secure-hash 'sha1 (plist-get r :pixels)))
        (and (plist-get r :mask) (secure-hash 'sha1 (plist-get r :mask)))))

(defun workers-image-gui--params (file bg)
  "The core's params for what the loader reads from this frame."
  (let* ((type (workers-image-gui--type file))
         (fc (color-values (frame-parameter nil 'background-color)))
         (c (and bg (color-values bg)))
         (px (lambda (v) (logior (ash (ash (nth 0 v) -8) 16)
                                 (ash (ash (nth 1 v) -8) 8)
                                 (ash (nth 2 v) -8)))))
    (append '(:limit 10000)
            (and (memq type '(png webp)) (list :bg (or c fc)))
            (and (eq type 'gif)
                 (append (list :frame-bg (funcall px fc))
                         (and c (list :gif-bg (funcall px c))))))))

(defun workers-image-gui--load (spec legacy)
  "SPEC loaded by the loader (LEGACY) or the loader under the flag.
Value: (DIGEST LISP-DATA CORNERS FAILED LOG)."
  (let ((nidhogg-workers--test-image-legacy legacy)
        (max-image-size 10000)
        start r)
    ;; *Messages* folds a repeated message into "[2 times]": break the run.
    (message "-- load %s" (cl-incf workers-image-gui--serial))
    (with-current-buffer (get-buffer-create "*Messages*")
      (setq start (point-max)))
    (setq r (nidhogg-workers--test-image-pixels spec))
    (list (workers-image-gui--digest r) (plist-get r :lisp-data)
          (plist-get r :corners) (plist-get r :failed)
          (with-current-buffer "*Messages*"
            (string-replace workers-image-gui--dir "@DIR@/"
                            (string-trim (buffer-substring start (point-max))))))))

(ert-deftest workers-gui/image-oracle ()
  ;; Today's loaders, the synchronous loaders under the flag and the
  ;; core give identical pixels, masks, metadata and error text.
  (skip-unless (display-graphic-p))
  (let* ((cases (workers-image-gui--read "oracle-cases.eld"))
         (errors (workers-image-gui--read "oracle-errors.eld"))
         (fc (color-values (frame-parameter nil 'background-color)))
         legacy flagged results texts)
    (nidhogg-workers--test-image-cache-clear)
    (dolist (c cases)
      (push (workers-image-gui--load
             (workers-image-gui--spec (nth 0 c) (nth 1 c) (nth 2 c)) t)
            legacy))
    (setq legacy (nreverse legacy))
    (dolist (c cases)
      (push (workers-image-gui--load
             (workers-image-gui--spec (nth 0 c) (nth 1 c) (nth 2 c)) nil)
            flagged))
    (setq flagged (nreverse flagged))
    (nidhogg-workers--test-image-cache-clear)
    (cl-loop for c in cases for l in legacy for f in flagged
             do (should-not (nth 3 l))
             (should (equal (nth 0 l) (nth 0 f)))
             (should (equal (nth 1 l) (nth 1 f)))
             (should (equal (nth 2 l) (nth 2 f)))
             (let ((r (nidhogg-workers--test-image-decode
                       (workers-image-gui--spec (nth 0 c) nil (nth 2 c))
                       (workers-image-gui--params (nth 0 c) (nth 1 c)))))
               (should (equal (workers-image-gui--digest r) (nth 0 l))))
             (push (cons c (nth 0 l)) results))
    ;; Errors: the same text and a failed image under both.
    (dolist (e errors)
      (pcase-let ((`(,file ,bg ,index) (if (consp e) e (list e nil nil))))
        (let* ((spec (workers-image-gui--spec file bg index))
               (l (workers-image-gui--load spec t))
               (f (workers-image-gui--load spec nil)))
          (should (nth 3 l))
          (should (nth 3 f))
          (should (equal (nth 4 l) (nth 4 f)))
          (push (cons (list file bg index) (nth 4 l)) texts))))
    (when-let ((out (getenv "NH_RECORD_ORACLE")))
      (let ((golden
             (concat ";; -*- lexical-binding: t -*-\n"
                     ";; Recorded from the loaders (nidhogg-workers--test-image-legacy t) by\n"
                     ";; workers-gui/image-oracle with NH_RECORD_ORACLE set.\n"
                     (format "((frame-bg-color %S)\n (results\n" fc)
                     (mapconcat (lambda (r) (format "  %S\n" r))
                                (reverse results) "")
                     "  ))\n"))
            (errors-golden
             (concat ";; -*- lexical-binding: t -*-\n"
                     ";; Recorded from the loaders by workers-gui/image-oracle with\n"
                     ";; NH_RECORD_ORACLE set: ((FILE BG INDEX) . LOG-TEXT).\n(\n"
                     (mapconcat (lambda (r) (format " %S\n" r))
                                (reverse texts) "")
                     ")\n")))
        (if (equal out "stderr")
            ;; The container mounts the checkout read-only: print the files.
            (let ((standard-output #'external-debugging-output))
              (princ (concat "\n@@NH-GOLDEN-BEGIN oracle-golden.eld\n" golden
                             "@@NH-GOLDEN-END\n@@NH-GOLDEN-BEGIN oracle-errors-golden.eld\n"
                             errors-golden "@@NH-GOLDEN-END\n")))
          (with-temp-file (expand-file-name "oracle-golden.eld" out)
            (insert golden))
          (with-temp-file (expand-file-name "oracle-errors-golden.eld" out)
            (insert errors-golden)))))))

;;; Task 29: pending entries, the display path, landing.  The rows wait
;;; on events (pause points, `--test-await-landed'), never on time.

(defmacro workers-image-gui--skip ()
  ;; A macro: `skip-unless' exists only inside an `ert-deftest' body.
  '(skip-unless (and (display-graphic-p)
                     (fboundp 'nidhogg-workers--deferrable-p)
                     (nidhogg-workers--deferrable-p 'png))))

(defun workers-image-gui--info (key)
  (plist-get (nidhogg-workers--test-image-info) key))

(defun workers-image-gui--img-jobs ()
  (plist-get (plist-get (nidhogg-workers-stats) :nh-img) :jobs))

(defmacro workers-image-gui--with-defer (&rest body)
  "Run BODY with images deferred on nh-img, then release every decode."
  (declare (indent 0))
  `(let ((nidhogg-workers-images t)
         (nidhogg-image-defer t)
         (nidhogg-workers--mode 'threads))
     (unwind-protect
         (progn ,@body)
       (nidhogg-workers--release)
       (nidhogg-workers--test-release-on-wait nil)
       (nidhogg-workers--test-await-landed 'nh-img 5000)
       (redisplay t))))

(defun workers-image-gui--show (specs)
  "Show SPECS, one per line, alone in the selected frame; return its window."
  (let ((buf (get-buffer-create " *workers-image-gui*")))
    (with-current-buffer buf
      (erase-buffer)
      (dolist (s specs)
        (insert-image s "x")
        (insert "\n"))
      (goto-char (point-min)))
    (delete-other-windows)
    (switch-to-buffer buf)
    (selected-window)))

(defun workers-image-gui--pend (specs)
  "Show SPECS with nh-img parked at `after-header'; one redisplay.
Return the window."
  (nidhogg-workers--pause-at 'after-header)
  (clear-image-cache t)
  (prog1 (workers-image-gui--show specs)
    (redisplay t)
    (should (nidhogg-workers--test-await-paused 'after-header 5000 1))))

(defun workers-image-gui--land ()
  "Release the parked decodes and land them in one redisplay."
  (nidhogg-workers--release 'after-header)
  (should (nidhogg-workers--test-await-landed 'nh-img 5000))
  (redisplay t)
  (should (= 0 (workers-image-gui--info :pending))))

(ert-deftest workers-gui/image-defer ()
  ;; §17.2b M5.  A pending entry of the final size; layout unchanged when
  ;; the decode lands; the landing redraws exactly the windows showing
  ;; the image; `image-size' answers without waiting; `image-mask-p'
  ;; waits; `nidhogg-image-defer' nil gives today's behaviour.
  (workers-image-gui--skip)
  (workers-image-gui--with-defer
    (let* ((spec (append (workers-image-gui--spec "palette.png" nil nil)
                         '(:scale 2)))
           (deferred (workers-image-gui--info :deferred))
           (w1 (workers-image-gui--pend (list spec)))
           (h (window-line-height 0 w1)))
      ;; The other window shows no image: the landing must not mark it.
      (let ((w2 (split-window w1)))
        (set-window-buffer w2 (get-buffer-create " *workers-image-gui-other*"))
        (redisplay t)
        (should (= 1 (workers-image-gui--info :pending)))
        (should (= (1+ deferred) (workers-image-gui--info :deferred)))
        (should (>= (car h) 22))
        ;; The header answers: a wait would have landed the entry.
        (should (equal (image-size spec t) '(34 . 22)))
        (should (= 1 (workers-image-gui--info :pending)))
        (workers-image-gui--land)
        (should (equal (nidhogg-workers--test-marked-windows) (list w1)))
        (should (equal (window-line-height 0 w1) h))
        (delete-window w2)))
    ;; A caller that needs the pixels waits for the job.
    (let ((spec (workers-image-gui--spec "alpha.png" nil nil))
          (landed (workers-image-gui--info :landed)))
      (workers-image-gui--pend (list spec))
      (should (= 1 (workers-image-gui--info :pending)))
      (nidhogg-workers--test-release-on-wait 'after-header)
      (let ((mask (image-mask-p spec)))
        (should (= 0 (workers-image-gui--info :pending)))
        (should (= (1+ landed) (workers-image-gui--info :landed)))
        ;; The same answer as a synchronous load.
        (let ((nidhogg-workers-images nil))
          (clear-image-cache t)
          (should (eq mask (image-mask-p spec))))))
    ;; nidhogg-image-defer nil: today's path, no job.
    (let ((nidhogg-image-defer nil)
          (jobs (workers-image-gui--img-jobs)))
      (nidhogg-workers--pause-at 'after-header)
      (clear-image-cache t)
      (workers-image-gui--show
       (list (workers-image-gui--spec "palette.png" nil nil)))
      (redisplay t)
      (should (= 0 (workers-image-gui--info :pending)))
      (should (= jobs (workers-image-gui--img-jobs)))
      (nidhogg-workers--release))))

(ert-deftest workers-gui/image-frame-inputs ()
  ;; §17.2b S5.  A PNG with a real alpha channel and a GIF with a
  ;; transparent background, under two frame backgrounds and with
  ;; `:background': the landed pixels equal the synchronous path's for
  ;; each; a background change makes a new entry, as today.
  (workers-image-gui--skip)
  (skip-unless (nidhogg-workers--deferrable-p 'gif))
  (let ((old (frame-parameter nil 'background-color)))
    (unwind-protect
        (workers-image-gui--with-defer
          (dolist (theme '("white" "black"))
            (set-frame-parameter nil 'background-color theme)
            (dolist (c '(("alpha.png" nil) ("alpha.png" "#336699")
                         ("oracle-anim-transparent.gif" nil)
                         ("oracle-anim-transparent.gif" "#336699")))
              (ert-info ((format "%s %S %s" theme (car c) (cadr c)) :prefix "")
                (let* ((spec (workers-image-gui--spec (car c) (cadr c) nil))
                       (sync (workers-image-gui--digest
                              (nidhogg-workers--test-image-pixels spec))))
                  (workers-image-gui--pend (list spec))
                  (should (= 1 (workers-image-gui--info :pending)))
                  (workers-image-gui--land)
                  (should (equal (workers-image-gui--digest
                                  (nidhogg-workers--test-image-pixels spec nil t))
                                 sync))))))
          ;; The frame's background changes under a landed image: a new
          ;; entry, decoded again.
          (let ((spec (workers-image-gui--spec "alpha.png" nil nil)))
            (workers-image-gui--pend (list spec))
            (workers-image-gui--land)
            (nidhogg-workers--pause-at 'after-header)
            (set-frame-parameter nil 'background-color "white")
            (redisplay t)
            (should (nidhogg-workers--test-await-paused 'after-header 5000 1))
            (should (= 1 (workers-image-gui--info :pending)))
            (workers-image-gui--land)))
      (set-frame-parameter nil 'background-color old))))

(ert-deftest workers-gui/image-twenty-in-one-budget ()
  ;; S15.  A 20-image page with every decode parked: the budget ledger of
  ;; that one redisplay grants at most the budget in total and nothing to
  ;; every take after it; every image lands later.
  (workers-image-gui--skip)
  (workers-image-gui--with-defer
    (let ((specs (cl-loop for i from 1 to 20
                          collect (append (workers-image-gui--spec
                                           "palette.png" nil nil)
                                          (list :ascent (+ 50 i))))))
      (nidhogg-workers--pause-at 'after-header)
      (clear-image-cache t)
      (workers-image-gui--show specs)
      (nidhogg-workers--test-budget-ledger t)
      (redisplay t)
      (let ((ledger (nidhogg-workers--test-budget-ledger t)))
        (should (<= (car ledger) (* nidhogg-redisplay-wait-budget 1000000)))
        ;; The first image's last take and one take for each of the rest.
        (should (>= (cdr ledger) 20)))
      (should (= 20 (workers-image-gui--info :pending)))
      (nidhogg-workers--release 'after-header)
      (should (nidhogg-workers--test-await-landed 'nh-img 10000))
      (redisplay t)
      (should (= 0 (workers-image-gui--info :pending))))))

(ert-deftest workers-gui/image-evicted ()
  ;; §17.2b S5 through the real cache: `clear-image-cache', `image-flush'
  ;; and deleting a frame while the decode is parked.  The stale result is
  ;; dropped, nothing lands for it and its job is freed (ASan).  Eviction
  ;; by `image-cache-eviction-delay' goes through the same free_image
  ;; (workers/image-evicted); it needs the clock, so it has no row here.
  (workers-image-gui--skip)
  (workers-image-gui--with-defer
    (let ((spec (workers-image-gui--spec "palette.png" nil nil)))
      (dolist (how '(clear flush))
        (ert-info ((format "%s" how) :prefix "")
          (workers-image-gui--pend (list spec))
          (should (= 1 (workers-image-gui--info :pending)))
          (let ((dropped (workers-image-gui--info :dropped))
                (landed (workers-image-gui--info :landed)))
            (pcase how
              ('clear (clear-image-cache t))
              ('flush (image-flush spec t)))
            (should (= 0 (workers-image-gui--info :pending)))
            (should (= (1+ dropped) (workers-image-gui--info :dropped)))
            (workers-image-gui--show nil)
            (workers-image-gui--land)
            (should (= landed (workers-image-gui--info :landed)))
            (should (= 0 (workers-image-gui--info :live))))))
      ;; A frame that showed it is deleted: the entry is dropped with the
      ;; frame's cache, or lands in the cache the frames share.
      (let ((f1 (selected-frame))
            (f2 (make-frame)))
        (unwind-protect
            (progn
              (select-frame f2)
              (workers-image-gui--pend (list spec))
              (should (= 1 (workers-image-gui--info :pending)))
              (select-frame f1)
              (delete-frame f2)
              (workers-image-gui--land)
              (should (= 0 (workers-image-gui--info :live))))
          (select-frame f1)
          (when (frame-live-p f2)
            (delete-frame f2)))))))

(ert-deftest workers-gui/image-size-from-lisp-thread ()
  ;; Review Focus 2: a Lisp thread holding the global lock asks for the
  ;; size (the header answers) and for the mask (it waits for the job and
  ;; lands it), as the main thread would.
  (workers-image-gui--skip)
  (skip-unless (fboundp 'make-thread))
  (workers-image-gui--with-defer
    (let ((spec (workers-image-gui--spec "palette.png" nil nil)))
      (workers-image-gui--pend (list spec))
      (should (= 1 (workers-image-gui--info :pending)))
      ;; A thread does not see this thread's `let' bindings.
      (should (equal (thread-join
                      (make-thread (lambda ()
                                     (let ((nidhogg-workers-images t))
                                       (image-size spec t)))))
                     '(17 . 11)))
      (should (= 1 (workers-image-gui--info :pending)))
      (nidhogg-workers--test-release-on-wait 'after-header)
      (should-not (thread-join (make-thread (lambda () (image-mask-p spec)))))
      (should (= 0 (workers-image-gui--info :pending))))))

(ert-deftest workers-gui/image-lisp-display-waits ()
  ;; Review S3: display code outside redisplay (`window-text-pixel-size')
  ;; meets a new image with nh-img parked: it loads the image itself and
  ;; gets the decoded size; no pending entry, no job.
  (workers-image-gui--skip)
  (workers-image-gui--with-defer
    (let ((spec (append (workers-image-gui--spec "palette.png" nil nil)
                        '(:scale 2)))
          (jobs (workers-image-gui--img-jobs)))
      (nidhogg-workers--pause-at 'after-header)
      (clear-image-cache t)
      (let* ((w (workers-image-gui--show (list spec)))
             (size (window-text-pixel-size w)))
        (should (>= (car size) 34))
        (should (>= (cdr size) 22)))
      (should (= 0 (workers-image-gui--info :pending)))
      (should (= jobs (workers-image-gui--img-jobs))))))

;;; workers-image-gui-tests.el ends here
