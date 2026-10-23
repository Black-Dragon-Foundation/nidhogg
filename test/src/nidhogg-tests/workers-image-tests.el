;;; workers-image-tests.el --- tests for workers: image decoding  -*- lexical-binding: t -*-

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

(defconst workers-image-tests--dir
  (expand-file-name "fixtures/images/"
                    (file-name-directory (or load-file-name buffer-file-name)))
  "The image corpus.")

(defun workers-image-tests--read (name)
  "The Lisp data in fixtures/images/NAME."
  (with-temp-buffer
    (insert-file-contents (expand-file-name name workers-image-tests--dir))
    (goto-char (point-min))
    (read (current-buffer))))

(defun workers-image-tests--type (file)
  "The image type of FILE, by extension."
  (pcase (file-name-extension file)
    ("png" 'png) ("jpg" 'jpeg) ("gif" 'gif) ("webp" 'webp)))

(defun workers-image-tests--core-spec (file index)
  "The image spec of corpus FILE with INDEX."
  `(image :type ,(workers-image-tests--type file)
          :file ,(expand-file-name file workers-image-tests--dir)
          ,@(and index (list :index index))))

(defun workers-image-tests--params (file bg golden-header)
  "The core's params for FILE over background BG (a colour string or nil).
GOLDEN-HEADER is the corpus' recorded frame background colour."
  (let* ((type (workers-image-tests--type file))
         (frame-color (cadr (assq 'frame-bg-color golden-header)))
         (c (and bg (color-values bg)))
         (px (lambda (v) (logior (ash (ash (nth 0 v) -8) 16)
                                 (ash (ash (nth 1 v) -8) 8)
                                 (ash (nth 2 v) -8)))))
    (append '(:limit 10000)
            (and (memq type '(png webp))
                 (list :bg (or c frame-color)))
            (and (eq type 'gif)
                 (append (list :frame-bg (funcall px frame-color))
                         (and c (list :gif-bg (funcall px c))))))))

(defun workers-image-tests--digest (r)
  "(W H PIXELS-SHA1 MASK-SHA1) of the core's result R."
  (list (plist-get r :width) (plist-get r :height)
        (secure-hash 'sha1 (plist-get r :pixels))
        (and (plist-get r :mask) (secure-hash 'sha1 (plist-get r :mask)))))

(defun workers-image-tests--golden ()
  "The recorded results: ((FRAME-BG-COLOR C) (RESULTS . ALIST))."
  (workers-image-tests--read "oracle-golden.eld"))

(ert-deftest workers/image-oracle ()
  ;; Every corpus image through the core equals what today's loaders
  ;; produce (the recorded results; workers-gui/image-oracle holds the
  ;; loaders, the synchronous loaders under the flag and the core equal
  ;; live and re-records them).
  (let* ((golden (workers-image-tests--golden))
         (results (cdr (assq 'results golden)))
         (n 0))
    (should results)
    (nidhogg-workers--test-image-cache-clear)
    (dolist (case (workers-image-tests--read "oracle-cases.eld"))
      (pcase-let ((`(,file ,bg ,index) case))
        (let ((r (nidhogg-workers--test-image-decode
                  (workers-image-tests--core-spec file index)
                  (workers-image-tests--params file bg golden))))
          (should-not (plist-get r :error))
          (should (equal (workers-image-tests--digest r)
                         (cdr (assoc case results))))
          (cl-incf n))))
    (should (= n (length results)))))

(ert-deftest workers/image-animation-cache ()
  ;; Frame N decodes from the cached state at N-1: one frame decoded.
  (let ((golden (workers-image-tests--golden)))
    (dolist (file '("oracle-anim-200.gif" "oracle-anim-200.webp"))
      (nidhogg-workers--test-image-cache-clear)
      (let ((params (workers-image-tests--params file nil golden)))
        (dotimes (i 150)
          (should-not (plist-get (nidhogg-workers--test-image-decode
                                  (workers-image-tests--core-spec file i) params)
                                 :error)))
        (nidhogg-workers--test-decode-count t)
        (let ((r (nidhogg-workers--test-image-decode
                  (workers-image-tests--core-spec file 150) params)))
          (should (= 1 (nidhogg-workers--test-decode-count)))
          ;; The same pixels as the loaders'.
          (should (equal (workers-image-tests--digest r)
                         (cdr (assoc (list file nil 150)
                                     (cdr (assq 'results golden))))))
          ;; Without the cache: all 151 frames, same pixels.
          (nidhogg-workers--test-image-cache-clear)
          (nidhogg-workers--test-decode-count t)
          (should (equal (workers-image-tests--digest
                          (nidhogg-workers--test-image-decode
                           (workers-image-tests--core-spec file 150) params))
                         (workers-image-tests--digest r)))
          (should (= 151 (nidhogg-workers--test-decode-count))))))))

(defun workers-image-tests--error-text (file r &optional index)
  "The text the loader logs for the core's failure R on FILE."
  (let* ((type (workers-image-tests--type file))
         (path (expand-file-name file workers-image-tests--dir))
         (msg (plist-get r :message))
         (spec (workers-image-tests--core-spec file index)))
    (pcase (plist-get r :error)
      ('not-image (format "Not a PNG file: `%s'" path))
      ('lib (if (eq type 'png)
                (format "PNG error: %s" msg)
              (format "Error reading JPEG image `%s': %s" spec msg)))
      ('gif-open (if (equal msg "")
                     (format "Cannot open `%s'" path)
                   (format "Cannot open `%s': %s" path msg)))
      ('gif-read (if (equal msg "")
                     (format "Error reading `%s'" spec)
                   (format "Error reading `%s' (%s)" spec msg)))
      ('gif-index (format "Invalid image number `%s' in image `%s'"
                          (plist-get r :arg) spec))
      ('webp-headers (format "Error parsing WebP headers from file: `%s'" path))
      ('webp-parse (format "Error parsing WebP file: `%s'" path))
      ('webp-decode (format "Error decoding WebP file: `%s'" path))
      ('webp-frame (format "Error decoding frame #%d from WebP file: `%s'"
                           (plist-get r :arg) path))
      ('size "Invalid image size (see `max-image-size')")
      (e (format "%s" e)))))

(ert-deftest workers/image-errors ()
  ;; Truncated and corrupt images fail with the loaders' text.
  (let ((golden (workers-image-tests--golden))
        (texts (workers-image-tests--read "oracle-errors-golden.eld")))
    (should texts)
    (dolist (entry (workers-image-tests--read "oracle-errors.eld"))
      (pcase-let ((`(,file ,bg ,index) (if (consp entry) entry
                                         (list entry nil nil))))
        (let ((r (nidhogg-workers--test-image-decode
                  (workers-image-tests--core-spec file index)
                  (workers-image-tests--params file bg golden)))
              (want (string-replace ;; recorded in a GUI frame: curved quotes
                     "’" "'"
                     (string-replace
                      "‘" "`"
                      (cdr (assoc (list file bg index) texts))))))
          (should (plist-get r :error))
          (should-not (plist-get r :pixels))
          (should (equal (string-replace
                          workers-image-tests--dir "@DIR@/"
                          (concat (mapconcat
                                   (lambda (w) (format "PNG warning: %s\n" w))
                                   (plist-get r :warnings) "")
                                  (workers-image-tests--error-text file r index)))
                         want)))))))

(ert-deftest workers/image-data-copy ()
  ;; The job decodes a copy: the Lisp string is cleared and the heap
  ;; collected while the job is held, and the copy still decodes.
  (let* ((golden (workers-image-tests--golden))
         (file "oracle-rgb16.png")
         (data (with-temp-buffer
                 (set-buffer-multibyte nil)
                 (insert-file-contents-literally
                  (expand-file-name file workers-image-tests--dir))
                 (buffer-string)))
         (want (cdr (assoc (list file nil nil) (cdr (assq 'results golden)))))
         (h (nidhogg-workers--test-image-decode
             `(image :type png :data ,data)
             (workers-image-tests--params file nil golden) 'thread)))
    (should (natnump h))
    (clear-string data)
    (garbage-collect)
    (let ((r (nidhogg-workers--test-image-decode-finish h)))
      (should-not (plist-get r :error))
      (should (equal (workers-image-tests--digest r) want)))))

(defconst workers-image-tests--dir
  (expand-file-name "fixtures/images/"
                    (file-name-directory (or load-file-name buffer-file-name))))

(defconst workers-image-tests--corpus
  ;; (FILE TYPE WIDTH HEIGHT): the sizes the files were made with.
  '(("palette.png" png 17 11) ("depth16.png" png 23 13)
    ("interlace.png" png 19 15) ("gama.png" png 21 9) ("alpha.png" png 20 10)
    ("base420.jpg" jpeg 32 24) ("base444.jpg" jpeg 32 24)
    ("progressive.jpg" jpeg 32 24) ("cmyk.jpg" jpeg 24 32)
    ("exif6.jpg" jpeg 30 20)
    ("one.gif" gif 18 14) ("anim.gif" gif 12 10)
    ("lossy.webp" webp 25 13) ("lossless.webp" webp 25 13)
    ("alpha.webp" webp 25 13) ("lossy-alpha.webp" webp 26 14)
    ("anim.webp" webp 16 12))
  "The corpus of fixtures/images.")

(defun workers-image-tests--spec (file type &rest more)
  (append (list 'image :type type :file
                (expand-file-name file workers-image-tests--dir))
          more))

(defun workers-image-tests--data (file type n &rest more)
  "A :data spec of the first N bytes of FILE."
  (with-temp-buffer
    (set-buffer-multibyte nil)
    (insert-file-contents-literally
     (expand-file-name file workers-image-tests--dir))
    (append (list 'image :type type :data
                  (buffer-substring-no-properties
                   (point-min) (min (point-max) (+ (point-min) n))))
            more)))

(defun workers-image-tests--answer (kind spec)
  "The header answer for KIND of SPEC: (ANSWER), or nil when it does not answer."
  (let ((nidhogg-workers-images t))
    (nidhogg-workers--test-image-answer kind spec)))

(defun workers-image-tests--crc32 (string)
  (let ((c #xffffffff))
    (dolist (b (append string nil))
      (setq c (logxor c b))
      (dotimes (_ 8)
        (setq c (if (zerop (logand c 1)) (ash c -1)
                  (logxor (ash c -1) #xedb88320)))))
    (logxor c #xffffffff)))

(defun workers-image-tests--png-header (w h)
  "The first 33 bytes of a PNG of W x H pixels, with a valid IHDR."
  (let* ((be32 (lambda (n) (unibyte-string (logand (ash n -24) 255)
                                           (logand (ash n -16) 255)
                                           (logand (ash n -8) 255)
                                           (logand n 255))))
         (ihdr (concat "IHDR" (funcall be32 w) (funcall be32 h)
                       (unibyte-string 8 2 0 0 0))))
    (concat "\x89PNG\r\n\x1a\n" (funcall be32 13) ihdr
            (funcall be32 (workers-image-tests--crc32 ihdr)))))

(ert-deftest workers/image-header-answers ()
  ;; The size and the frame data of every corpus image come from the header
  ;; path, equal to what the loaders compute (the GUI row compares with the
  ;; loaders themselves).
  (dolist (c workers-image-tests--corpus)
    (pcase-let ((`(,file ,type ,w ,h) c))
      (let ((spec (workers-image-tests--spec file type)))
        (should (equal (workers-image-tests--answer 'size spec)
                       (list (cons w h))))
        (should (equal (workers-image-tests--answer
                        'size (workers-image-tests--data file type 100000))
                       (list (cons w h))))
        (unless (memq type '(gif webp))
          (should (equal (workers-image-tests--answer 'metadata spec)
                         '(nil)))))))
  ;; The transform: :width, :max-width, :scale, :rotation and :margin.
  (let ((png (lambda (&rest more)
               (workers-image-tests--answer
                'size (apply #'workers-image-tests--spec
                             "palette.png" 'png more)))))
    (should (equal (funcall png :width 34) '((34 . 22))))
    (should (equal (funcall png :max-width 8) '((8 . 6))))
    (should (equal (funcall png :height 22) '((34 . 22))))
    (should (equal (funcall png :scale 2) '((34 . 22))))
    (should (equal (funcall png :rotation 90) '((11 . 17))))
    (should (equal (funcall png :rotation 180) '((17 . 11))))
    (should (equal (funcall png :scale 2 :rotation 270) '((22 . 34))))
    (should (equal (funcall png :margin 3) '((23 . 17))))
    (should (equal (funcall png :margin '(1 . 2) :relief 1) '((21 . 17))))
    ;; A rotation the loader reports an error for is not answered.
    (should-not (funcall png :rotation 45)))
  ;; Frames.
  (let ((m (car (workers-image-tests--answer
                  'metadata (workers-image-tests--spec "anim.gif" 'gif)))))
    (should (equal (plist-get m 'count) 3))
    (should (equal (plist-get m 'delay) 0.07))
    (should (plist-get m 'extension-data)))
  (should (equal (workers-image-tests--answer
                  'metadata (workers-image-tests--spec "anim.webp" 'webp))
                 '((delay 0.12 count 3))))
  (should (equal (workers-image-tests--answer
                  'metadata (workers-image-tests--spec "lossy.webp" 'webp))
                 '(nil)))
  (should (equal (workers-image-tests--answer
                  'size (workers-image-tests--spec "anim.gif" 'gif :index 2))
                 '((12 . 10))))
  ;; A frame the loader would report as missing is not answered.
  (should-not (workers-image-tests--answer
               'size (workers-image-tests--spec "anim.gif" 'gif :index 3)))
  ;; A corrupt body behind a valid header: the header's size (the recorded
  ;; residual); the frame scans of a truncated file answer nothing.
  (should (equal (workers-image-tests--answer
                  'size (workers-image-tests--data "palette.png" 'png 33))
                 '((17 . 11))))
  (should (equal (workers-image-tests--answer
                  'size (workers-image-tests--data "progressive.jpg" 'jpeg 300))
                 '((32 . 24))))
  (should (equal (workers-image-tests--answer
                  'size (workers-image-tests--data "one.gif" 'gif 13))
                 '((18 . 14))))
  (should-not (workers-image-tests--answer
               'metadata (workers-image-tests--data "anim.gif" 'gif 100)))
  (should-not (workers-image-tests--answer
               'metadata (workers-image-tests--data "anim.webp" 'webp 100)))
  ;; Not an image of the type: today's path reports it.
  (should-not (workers-image-tests--answer
               'size (workers-image-tests--data "base420.jpg" 'png 1000)))
  ;; The switch is off: nothing answers.
  (let ((nidhogg-workers-images nil))
    (should-not (nidhogg-workers--test-image-answer
                 'size (workers-image-tests--spec "palette.png" 'png)))))

(ert-deftest workers/image-header-bomb ()
  ;; A 100000 x 100000 header is refused by `max-image-size' before any
  ;; decode: the header path does not answer, so today's path reports the
  ;; error.  Only the limit refuses it: without one the header is trusted.
  (let ((spec (list 'image :type 'png :data
                    (workers-image-tests--png-header 100000 100000))))
    (should-not (workers-image-tests--answer 'size spec))
    (let ((max-image-size nil))
      (should (equal (workers-image-tests--answer 'size spec)
                     '((100000 . 100000)))))
    (let ((max-image-size 200000))
      (should (equal (workers-image-tests--answer 'size spec)
                     '((100000 . 100000)))))
    ;; The same size in a GIF screen descriptor.
    (let ((gif (list 'image :type 'gif :data
                     (concat "GIF89a" (unibyte-string 160 134 160 134 0 0 0)))))
      (should-not (workers-image-tests--answer 'size gif)))))

(defun workers-image-tests--info (key)
  (plist-get (nidhogg-workers--test-image-info) key))

(ert-deftest workers/image-evicted ()
  ;; §17.2b S5, frame-less: display jobs for three test entries; two of
  ;; the entries go (what free_image does for every eviction,
  ;; `clear-image-cache', `image-flush' and frame deletion) while their
  ;; decode is parked at `after-header' (threads) or already done
  ;; (inline).  Their results are dropped by the entry's generation, the
  ;; third lands, and every job is freed (ASan in workers-asan).  The GUI
  ;; row workers-gui/image-evicted does it through the real cache.
  (skip-unless (fboundp 'nidhogg-workers--test-image-job))
  (let ((spec (workers-image-tests--spec "palette.png" 'png))
        (params '(:bg (65535 65535 65535) :limit 10000)))
    (dolist (mode '(threads inline))
      (ert-info ((format "mode %s" mode) :prefix "")
        (let ((nidhogg-workers--mode mode))
          (nidhogg-workers--test-await-landed 'nh-img 5000)
          (nidhogg-workers--test-image-land)
          (let ((dropped (workers-image-tests--info :dropped))
                (landed (workers-image-tests--info :landed)))
            (unwind-protect
                (progn
                  (when (eq mode 'threads)
                    (nidhogg-workers--pause-at 'after-header))
                  (dotimes (i 3)
                    (should (equal (nidhogg-workers--test-image-job
                                    spec (1+ i) params)
                                   (1+ i))))
                  (when (eq mode 'threads)
                    (should (nidhogg-workers--test-await-paused
                             'after-header 5000 1)))
                  (should (= 3 (workers-image-tests--info :pending)))
                  (should (nidhogg-workers--test-image-forget 1))
                  (should (nidhogg-workers--test-image-forget 2))
                  (should-not (nidhogg-workers--test-image-forget 2))
                  (should (= 1 (workers-image-tests--info :pending)))
                  (nidhogg-workers--release 'after-header)
                  (should (nidhogg-workers--test-await-landed 'nh-img 5000))
                  (should (= 1 (workers-image-tests--info :landed-waiting)))
                  (should (= 1 (nidhogg-workers--test-image-land)))
                  (should (= 0 (workers-image-tests--info :pending)))
                  (should (= (+ dropped 2) (workers-image-tests--info :dropped)))
                  (should (= (+ landed 1) (workers-image-tests--info :landed)))
                  (should (= 0 (workers-image-tests--info :live))))
              (nidhogg-workers--release)
              (dolist (i '(1 2 3))
                (nidhogg-workers--test-image-forget i)))))))))

;; macOS keeps NSImage for its native types (Task 37): they decode
;; synchronously there, so none of them is deferrable.  WebP is not
;; asserted: whether it reaches nh-img on NS is decided by the Mac build.
(ert-deftest workers/macos-image-types ()
  (skip-unless (and (eq system-type 'darwin)
                    (fboundp 'nidhogg-workers--deferrable-p)))
  (dolist (type '(png jpeg gif tiff))
    (should-not (nidhogg-workers--deferrable-p type))))

;;; workers-image-tests.el ends here
