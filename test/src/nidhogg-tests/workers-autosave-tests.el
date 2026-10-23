;;; workers-autosave-tests.el --- tests for workers: auto-save  -*- lexical-binding: t -*-

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

;;; Commentary:

;; Auto-save writes on nh-save (Task 34, §8.6).  The keyboard loop's
;; auto-save is `nidhogg-workers--test-keyboard-auto-save', the call
;; `read_char' makes after typing.  Most rows compare a run with
;; `nidhogg-workers-autosave' off (today's synchronous write) with the
;; same run with it on: in `threads' mode the job is parked at
;; `before-write' while the race happens, in `inline' mode it has run
;; but not landed.  Rows that need a parked job run in `threads' mode
;; only.  Every wait is a bounded condition wait
;; (`nidhogg-workers--test-save-await', `--test-await-paused').

;;; Code:

(require 'ert)
(require 'cl-lib)

(defun workers-autosave-tests--size ()
  "The big buffer's size: 50 MB, scaled by NIDHOGG_WORKERS_TEST_SIZE_SCALE."
  (max 100000
       (round (* 50 1024 1024
                 (string-to-number
                  (or (getenv "NIDHOGG_WORKERS_TEST_SIZE_SCALE") "1"))))))

(defmacro workers-autosave-tests--in-dir (dir &rest body)
  "Run BODY with DIR bound to a new temporary directory (a directory name).
Afterwards release every pause point, land every job, kill the buffers
visiting files in DIR and delete it."
  (declare (indent 1) (debug t))
  `(let ((,dir (file-name-as-directory (make-temp-file "nh-wk-save" t))))
     (unwind-protect (progn ,@body)
       (nidhogg-workers--release)
       (nidhogg-workers--test-save-await 10000)
       (dolist (b (buffer-list))
         (when (and (buffer-file-name b)
                    (string-prefix-p ,dir (buffer-file-name b)))
           (with-current-buffer b
             (clear-buffer-auto-save-failure)
             (set-buffer-modified-p nil)
             (let ((kill-buffer-query-functions nil)
                   (kill-buffer-hook nil))
               (kill-buffer b)))))
       (delete-directory ,dir t))))

(defun workers-autosave-tests--visit (file text)
  "Visit FILE in a new buffer, insert TEXT, turn on auto-saving; return the buffer."
  (let ((buf (find-file-noselect file)))
    (with-current-buffer buf
      (insert text)
      (auto-save-mode 1))
    buf))

(defun workers-autosave-tests--bytes (file)
  "FILE's bytes as a unibyte string, or nil if it does not exist."
  (and (file-exists-p file)
       (with-temp-buffer
         (set-buffer-multibyte nil)
         (insert-file-contents-literally file)
         (buffer-string))))

(defun workers-autosave-tests--files (dir)
  "The regular files in DIR that do not start with a dot, with their bytes."
  (mapcar (lambda (f) (cons f (workers-autosave-tests--bytes
                               (expand-file-name f dir))))
          (cl-remove-if-not (lambda (f) (file-regular-p (expand-file-name f dir)))
                            (directory-files dir nil "\\`[^.]"))))

(defun workers-autosave-tests--parked-p (async)
  "Non-nil when a job of an ASYNC run parks at `before-write'."
  (and async (eq nidhogg-workers--mode 'threads)))

(defun workers-autosave-tests--autosave (async)
  "The keyboard-triggered auto-save of an ASYNC run (or a synchronous one).
In `threads' mode the job parks at `before-write'."
  (let ((nidhogg-workers-autosave async))
    (when (workers-autosave-tests--parked-p async)
      (nidhogg-workers--pause-at 'before-write))
    (nidhogg-workers--test-keyboard-auto-save t)
    (when (workers-autosave-tests--parked-p async)
      (should (nidhogg-workers--test-await-paused 'before-write 5000)))))

(defmacro workers-autosave-tests--warnings (var &rest body)
  "Run BODY, pushing the arguments of every `display-warning' onto VAR."
  (declare (indent 1) (debug t))
  `(cl-letf (((symbol-function 'display-warning)
              (lambda (&rest args) (push args ,var))))
     ,@body))

(defun workers-autosave-tests--messages-since (pos)
  "The text logged in the messages buffer after POS."
  (with-current-buffer (messages-buffer)
    (buffer-substring-no-properties (min pos (point-max)) (point-max))))

(defun workers-autosave-tests--text ()
  "Text for the big buffer: lines of ASCII, Latin, CJK, emoji, a character
above Unicode and raw bytes, about 1 KiB."
  (let ((line (concat "plain ASCII text, é ü 漢字 😀 "
                      (string #x3fff7f #x110000)
                      (string-to-multibyte (unibyte-string #x80 #xc0 #xff))
                      "\n")))
    (apply #'concat (make-list (/ 1024 (string-bytes line)) line))))

(defun workers-autosave-tests--typed (async dir &optional unnamed)
  "Type into a big buffer in DIR until auto-save fires; return what is seen.
ASYNC non-nil: with `nidhogg-workers-autosave'.  UNNAMED nil: the job's
named temporary file.  The value is (BYTES COUNTERS MESSAGES PARKED),
PARKED what was on disk while the job was parked."
  (let* ((nidhogg-workers--save-unnamed unnamed)
         (file (expand-file-name "big.txt" dir))
         (auto (expand-file-name "#big.txt#" dir))
         (chunk (workers-autosave-tests--text))
         (buf (workers-autosave-tests--visit file ""))
         parked pos)
    (with-current-buffer buf
      (while (< (buffer-size) (workers-autosave-tests--size))
        (insert chunk))
      (setq pos (with-current-buffer (messages-buffer) (point-max)))
      (let ((nidhogg-workers-autosave async))
        (when (workers-autosave-tests--parked-p async)
          (nidhogg-workers--pause-at 'before-write))
        (nidhogg-workers--test-keyboard-auto-save nil)
        (when (workers-autosave-tests--parked-p async)
          (should (nidhogg-workers--test-await-paused 'before-write 5000))
          (setq parked (list (file-exists-p auto)
                             (nidhogg-workers--test-save-jobs)))
          (nidhogg-workers--release 'before-write)))
      (should (nidhogg-workers--test-save-await 30000))
      (list (workers-autosave-tests--bytes auto)
            (list (recent-auto-save-p) buffer-saved-size
                  (buffer-modified-p))
            (workers-autosave-tests--messages-since pos)
            parked))))

(ert-deftest workers/autosave-async ()
  ;; §17.2b.  Typing into a 50 MB buffer until auto-save fires: with the
  ;; job parked at before-write the counters and messages are today's
  ;; and the auto-save file is not there yet; on release it equals the
  ;; synchronous one byte for byte (raw-byte characters included); no
  ;; temporary file is left, unnamed or named.
  (dolist (mode '(threads inline))
    (dolist (unnamed '(t nil))
      (ert-info ((format "mode %s, unnamed %s" mode unnamed) :prefix "")
        (let ((nidhogg-workers--mode mode)
              sync async)
          (workers-autosave-tests--in-dir dir
            (setq sync (workers-autosave-tests--typed nil dir unnamed)))
          (workers-autosave-tests--in-dir dir
            (setq async (workers-autosave-tests--typed t dir unnamed))
            (should-not (directory-files dir nil "\\.nh-")))
          (should (nth 0 sync))
          (should (> (length (nth 0 sync)) (workers-autosave-tests--size)))
          (should (equal (nth 0 async) (nth 0 sync)))
          (should (equal (nth 1 async) (nth 1 sync)))
          (should (equal (nth 2 async) (nth 2 sync)))
          (when (eq mode 'threads)
            (should (equal (car (nth 3 async)) nil))
            (should (equal (mapcar #'cadr (cadr (nth 3 async)))
                           '(running))))))))
  ;; A write error shows today's warning (at the landing): the auto-save
  ;; name is in a directory that does not exist.
  (dolist (mode '(threads inline))
    (ert-info ((format "write error, mode %s" mode) :prefix "")
      (let ((nidhogg-workers--mode mode)
            (runs nil))
        (dolist (async '(nil t))
          (workers-autosave-tests--in-dir dir
            (let ((warnings nil)
                  (buf (workers-autosave-tests--visit
                        (expand-file-name "e.txt" dir) "text\n")))
              (with-current-buffer buf
                (setq buffer-auto-save-file-name
                      (expand-file-name "missing/#e.txt#" dir))
                (workers-autosave-tests--warnings warnings
                  (let ((nidhogg-workers-autosave async))
                    (nidhogg-workers--test-keyboard-auto-save t)
                    (should (nidhogg-workers--test-save-await 5000)))))
              (should-not (directory-files dir nil "\\.nh-"))
              (push (mapcar (lambda (w)
                              (replace-regexp-in-string
                               (regexp-quote dir) "DIR" (cadr w)))
                            warnings)
                    runs))))
        (should (car runs))
        (should (equal (car runs) (cadr runs)))))))

(defun workers-autosave-tests--race (async scenario)
  "Run SCENARIO in a buffer visiting a.txt in a new directory.
SCENARIO is called with the directory and a function that auto-saves as
the keyboard loop does (an ASYNC run's job parks or stays unlanded).
Then every job is released and landed.  Return the regular files with
their bytes, whether the auto-save file is newer than the visited file
\(`after-find-file's \"has auto save data\"), and SCENARIO's value."
  (workers-autosave-tests--in-dir dir
    (let* ((file (expand-file-name "a.txt" dir))
           (buf (workers-autosave-tests--visit file ""))
           (value (with-current-buffer buf
                    (funcall scenario dir
                             (lambda ()
                               (workers-autosave-tests--autosave async))))))
      (nidhogg-workers--release)
      (should (nidhogg-workers--test-save-await 5000))
      (should-not (directory-files dir nil "\\.nh-"))
      (list (workers-autosave-tests--files dir)
            (and (buffer-live-p buf)
                 (with-current-buffer buf
                   (and buffer-auto-save-file-name buffer-file-name
                        (file-newer-than-file-p buffer-auto-save-file-name
                                                buffer-file-name))))
            value))))

(defun workers-autosave-tests--sync-autosave ()
  "A synchronous keyboard-triggered auto-save (an older auto-save file)."
  (let ((nidhogg-workers-autosave nil))
    (nidhogg-workers--test-keyboard-auto-save t)))

(defconst workers-autosave-tests--scenarios
  `((save-buffer
     . ,(lambda (_dir go)
          (insert "one\n")
          (funcall go)
          (insert "two\n")
          (save-buffer)))
    (undo-to-unmodified
     . ,(lambda (_dir go)
          (insert "one\n")
          (save-buffer)
          (undo-boundary)
          (insert "two\n")
          (funcall go)
          (let ((last-command nil))
            (undo))
          (buffer-modified-p)))
    (set-visited-file-name
     . ,(lambda (dir go)
          (insert "one\n")
          (funcall go)
          (set-visited-file-name (expand-file-name "b.txt" dir) t)
          buffer-auto-save-file-name))
    (write-file
     . ,(lambda (dir go)
          (insert "one\n")
          (funcall go)
          (write-file (expand-file-name "c.txt" dir))))
    (delete-file
     . ,(lambda (_dir go)
          (insert "one\n")
          (workers-autosave-tests--sync-autosave)
          (insert "two\n")
          (funcall go)
          (delete-file buffer-auto-save-file-name)))
    (trash
     . ,(lambda (dir go)
          (insert "one\n")
          (workers-autosave-tests--sync-autosave)
          (insert "two\n")
          (funcall go)
          (let ((delete-by-moving-to-trash t)
                (trash-directory (expand-file-name "trash/" dir)))
            (delete-file buffer-auto-save-file-name t))
          (file-exists-p buffer-auto-save-file-name)))
    (kill-buffer
     . ,(lambda (_dir go)
          (insert "one\n")
          (funcall go)
          (let ((prompts nil)
                (kill-buffer-delete-auto-save-files t))
            (cl-letf (((symbol-function 'yes-or-no-p)
                       (lambda (p) (push p prompts) nil)))
              (let ((kill-buffer-query-functions nil))
                (kill-buffer)))
            prompts)))
    (do-auto-save
     ;; Not modified since: the real name holds the text at once.
     . ,(lambda (_dir go)
          (insert "one\n")
          (funcall go)
          (do-auto-save t)
          (workers-autosave-tests--bytes buffer-auto-save-file-name)))
    (do-auto-save-modified
     ;; Modified since: the synchronous write wins over the job.
     . ,(lambda (_dir go)
          (insert "one\n")
          (funcall go)
          (insert "two\n")
          (do-auto-save t)
          (workers-autosave-tests--bytes buffer-auto-save-file-name))))
  "The races of `workers/autosave-save-race': (NAME . SCENARIO).")

(defun workers-autosave-tests--child (mode file)
  "Run a child Nidhogg in MODE that has a job for FILE in flight at `kill-emacs'.
Return (STATUS CODE AUTO-SAVED-BYTES)."
  (let* ((form
          `(progn
             (setq nidhogg-workers--mode ',mode
                   nidhogg-workers-autosave t)
             (find-file ,file)
             (insert "killed with a job in flight")
             (auto-save-mode 1)
             ,@(when (eq mode 'threads)
                 '((nidhogg-workers--pause-at 'before-write)))
             (nidhogg-workers--test-keyboard-auto-save t)
             ,@(when (eq mode 'threads)
                 '((unless (nidhogg-workers--test-await-paused
                            'before-write 5000)
                     (kill-emacs 3))))
             (kill-emacs)))
         (proc (make-process
                :name "nh-wk-save" :buffer nil :noquery t
                :command (list (expand-file-name invocation-name
                                                 invocation-directory)
                               "-Q" "--batch" "--eval"
                               (prin1-to-string form))))
         (start (float-time)))
    (while (and (process-live-p proc) (< (- (float-time) start) 30))
      (accept-process-output proc 0.05))
    (when (process-live-p proc)
      (delete-process proc))
    (list (process-status proc) (process-exit-status proc)
          (workers-autosave-tests--bytes
           (expand-file-name (concat "#" (file-name-nondirectory file) "#")
                             (file-name-directory file))))))

(ert-deftest workers/autosave-save-race ()
  ;; §17.2b B3.  Each race with a job in flight (parked at before-write
  ;; in `threads' mode; run but not landed in `inline' mode), then
  ;; released: the files left, the "has auto save data" test and what
  ;; the race saw equal the synchronous run's.  save-buffer leaves no
  ;; auto-save file; undo to unmodified the same; set-visited-file-name
  ;; and write-file land under the new name only; delete-file and a
  ;; trash move drop the job; kill-buffer asks exactly as the
  ;; synchronous run; do-auto-save from Lisp writes the current text at
  ;; once and the job never overwrites it.
  (dolist (mode '(threads inline))
    (dolist (s workers-autosave-tests--scenarios)
      (ert-info ((format "mode %s, %s" mode (car s)) :prefix "")
        (let* ((nidhogg-workers--mode mode)
               (sync (workers-autosave-tests--race nil (cdr s)))
               (async (workers-autosave-tests--race t (cdr s))))
          (should (equal async sync))
          (pcase (car s)
            ('save-buffer
             (should-not (assoc "#a.txt#" (car async)))
             (should-not (nth 1 async)))
            ('set-visited-file-name
             (should (assoc "#b.txt#" (car async)))
             (should-not (assoc "#a.txt#" (car async))))
            ('kill-buffer
             (should (equal (nth 2 async) '("Delete auto-save file? "))))
            ('do-auto-save
             (should (equal (nth 2 async) "one\n")))
            ('do-auto-save-modified
             (should (equal (nth 2 async) "one\ntwo\n"))
             (should (equal (cdr (assoc "#a.txt#" (car async)))
                            "one\ntwo\n"))))))))
  ;; kill-emacs with a job in flight: the real name holds the text.
  (dolist (mode '(threads inline))
    (ert-info ((format "kill-emacs, mode %s" mode) :prefix "")
      (workers-autosave-tests--in-dir dir
        (should (equal (workers-autosave-tests--child
                        mode (expand-file-name "k.txt" dir))
                       '(exit 0 "killed with a job in flight")))
        (should-not (directory-files dir nil "\\.nh-"))))))

(ert-deftest workers/autosave-hung ()
  ;; §17.2b S9, `threads' mode (parked jobs).
  (let ((nidhogg-workers--mode 'threads)
        (nidhogg-workers-autosave t))
    (workers-autosave-tests--in-dir dir
      (let* ((file (expand-file-name "h.txt" dir))
             (auto (expand-file-name "#h.txt#" dir))
             (buf (workers-autosave-tests--visit file "one\n"))
             (warnings nil))
        (with-current-buffer buf
          (workers-autosave-tests--warnings warnings
            ;; A job parked past the next auto-save: that one is written
            ;; synchronously, with the warning; the parked job is dropped.
            (workers-autosave-tests--autosave t)
            (insert "two\n")
            (nidhogg-workers--test-keyboard-auto-save t)
            (should (equal (workers-autosave-tests--bytes auto) "one\ntwo\n"))
            (should (equal (length warnings) 1))
            (should (string-match-p
                     "\\`Auto-saving h\\.txt: The previous auto-save is still"
                     (cadr (car warnings))))
            (should (equal (mapcar #'cadr (nidhogg-workers--test-save-jobs))
                           '(dropped)))
            (nidhogg-workers--release 'before-write)
            (should (nidhogg-workers--test-save-await 5000))
            (should (equal (workers-autosave-tests--bytes auto) "one\ntwo\n"))
            ;; A job running over 60 s: the next auto-save writes nothing,
            ;; warns, and the buffer is skipped for 20 minutes as after a
            ;; 60 s write today; the job still lands.
            (setq warnings nil)
            (insert "three\n")
            (workers-autosave-tests--autosave t)
            (nidhogg-workers--test-save-age 61)
            (insert "four\n")
            (nidhogg-workers--test-keyboard-auto-save t)
            (should (equal (length warnings) 1))
            (should (string-match-p "not finished in 60 seconds"
                                    (cadr (car warnings))))
            (should (equal (workers-autosave-tests--bytes auto) "one\ntwo\n"))
            (insert "five\n")
            (nidhogg-workers--test-keyboard-auto-save t)
            (should (equal (length warnings) 1))
            (should (equal (mapcar #'cadr (nidhogg-workers--test-save-jobs))
                           '(running)))
            (nidhogg-workers--release 'before-write)
            (should (nidhogg-workers--test-save-await 5000))
            (should (equal (workers-autosave-tests--bytes auto)
                           "one\ntwo\nthree\n"))
            (clear-buffer-auto-save-failure)
            ;; Both directory slots of nh-blk hung: the auto-save, on its
            ;; own thread, still lands.
            (nidhogg-workers--pause-at 'before-done)
            (nidhogg-workers--test-job 'nh-blk 0 :class 'dir :pause 'before-done)
            (nidhogg-workers--test-job 'nh-blk 0 :class 'dir :pause 'before-done)
            (should (nidhogg-workers--test-await-paused 'before-done 5000 2))
            (insert "six\n")
            (nidhogg-workers--test-keyboard-auto-save t)
            (should (nidhogg-workers--test-save-await 5000))
            (should (equal (workers-autosave-tests--bytes auto)
                           "one\ntwo\nthree\nfour\nfive\nsix\n"))
            (nidhogg-workers--release 'before-done)))))))

(defun workers-autosave-tests--random-string (n)
  "A random multibyte string of N characters, raw bytes included.
Built from a list of characters: `aset' refuses a non-ASCII character
in a multibyte string, and `string' of ASCII alone is unibyte."
  (string-to-multibyte
   (apply #'string
          (mapcar (lambda (_)
                    (pcase (random 7)
                      (0 (random 128))
                      (1 (+ #x80 (random #x780)))
                      (2 (+ #x800 (random #xd000)))
                      (3 (+ #x10000 (random #x100000)))
                      (4 (+ #x110000 (random (- #x3fff80 #x110000))))
                      (5 (+ #x3fff80 (random 128)))
                      (_ (aref "\n\r\t é" (random 5)))))
                  (make-list n nil)))))

(ert-deftest workers/autosave-encoder-differential ()
  ;; The job's encoder (a copy of encode_coding_utf_8's rule) against
  ;; Emacs's: random multibyte text with raw-byte characters and
  ;; characters above Unicode against `encode-coding-string' with
  ;; `utf-8-emacs-unix'; random unibyte text (C0/C1 pairs included)
  ;; against `write-region' from a unibyte buffer with that coding.
  (random "workers-autosave")
  (dotimes (_ 300)
    (let ((s (workers-autosave-tests--random-string (random 200))))
      (should (equal (nidhogg-workers--test-save-encode s)
                     (encode-coding-string s 'utf-8-emacs-unix)))))
  (let ((f (make-temp-file "nh-wk-enc")))
    (unwind-protect
        (dotimes (_ 100)
          (let ((u (apply #'unibyte-string
                          (mapcar (lambda (_)
                                    (if (zerop (random 4))
                                        (aref [#xc0 #xc1 #x80 #xbf] (random 4))
                                      (random 256)))
                                  (make-list (random 200) nil)))))
            (with-temp-buffer
              (set-buffer-multibyte nil)
              (insert u)
              (let ((coding-system-for-write 'utf-8-emacs-unix))
                (write-region nil nil f nil 'silent)))
            (should (equal (nidhogg-workers--test-save-encode u)
                           (workers-autosave-tests--bytes f)))))
      (delete-file f))))

;;; workers-autosave-tests.el ends here
