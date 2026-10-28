;;; wl-trace-check.el --- protocol rules over a WAYLAND_DEBUG=client trace  -*- lexical-binding: t -*-

;; The native Wayland shell's tests on a real compositor (sway, mutter) cannot read the compositor's commit log, so
;; they check what the client itself said.  This file reads a `WAYLAND_DEBUG=client' trace and returns the violations
;; of the rules it was asked for, each with its line number.  gui.sh runs it after a session (`--trace-check RULES').
;;
;;   nidhogg -Q --batch -l wl-trace-check.el -f wl-trace-check-batch TRACE RULES
;;
;; RULES is a comma list of: errors acks unconfigured subsurface popup sync grab popup-order dnd.  Exit 0 no
;; violation, 1 violations, 2 usage.  Lines that are not trace lines (the test output shares the file) are ignored,
;; except by `errors'.
;;
;;   grab         every xdg_popup.grab serial came from a press (button, key, touch down) the client saw, and the
;;                popup's parent is a toplevel or the topmost grabbing popup
;;   popup-order  popups are destroyed topmost first: none before a popup that was made on it
;;   dnd          no wl_data_offer.finish without a non-zero set_actions since the offer's last enter; no receive
;;                after finish or destroy; wl_data_device.start_drag names a press serial whose release was not seen

;;; Code:

(require 'cl-lib)
(require 'subr-x)

(defconst wl-trace-check-rules '("errors" "acks" "unconfigured" "subsurface" "popup" "sync" "grab" "popup-order" "dnd")
  "The rule names `wl-trace-check-file' knows.")

(defvar wl-trace-check-counts nil
  "After `wl-trace-check-file': an alist (KEY . N) of how often the trace said what the rules look at.")

(defvar wl-trace-check-sample nil
  "After `wl-trace-check-file': the first lines of the file that were not trace lines (for a trace that matched nothing).")

(defconst wl-trace-check-window 100.0
  "How long (ms of trace time) a rule lets a follow-up commit or ack take.")

(defconst wl-trace-check--line-re
  (concat "^\\[ *\\([0-9:.]+\\)\\] *\\(?:{[^}]*}\\)? *\\(-> \\)?"
          "\\([a-z_0-9]+\\)[@#]\\([0-9]+\\)\\.\\([a-z_0-9]+\\)(\\(.*\\))$")
  "A libwayland client trace line: time, queue name, direction, object (`iface#id', older libwayland `iface@id'),
message, arguments.  The time is `HH:MM:SS.microseconds' (libwayland 1.24 and later) or milliseconds.")

(defconst wl-trace-check--error-re
  "Lost connection\\|connection broke\\|Protocol error\\|Broken pipe\\|wl_display\\.error"
  "Non-trace lines that mean the connection died.")

(defun wl-trace-check--ms (time)
  "TIME, a trace timestamp (\"HH:MM:SS.uuuuuu\" or \"SECONDS.MS\" in ms), as milliseconds."
  (let ((parts (save-match-data (split-string time ":"))))
    (if (cdr parts)
        (* 1000.0 (+ (* 3600 (string-to-number (nth 0 parts))) (* 60 (string-to-number (nth 1 parts)))
                     (string-to-number (nth 2 parts))))
      (string-to-number time))))

(defun wl-trace-check--refs (args)
  "The (INTERFACE . ID) pairs named in ARGS, in order (`new id' ones included)."
  (let ((pos 0) refs)
    (while (string-match "\\([a-z_0-9]+\\)[@#]\\([0-9]+\\)" args pos)
      (push (cons (match-string 1 args) (string-to-number (match-string 2 args))) refs)
      (setq pos (match-end 0)))
    (nreverse refs)))

(defun wl-trace-check--press (press released serial key down kind)
  "Note in PRESS the press of KEY (of KIND, with SERIAL) when DOWN; otherwise mark every press of KEY in RELEASED."
  (if down
      (progn (puthash serial (cons kind key) press)
             (remhash serial released))
    (maphash (lambda (s v) (when (equal (cdr v) key) (puthash s t released))) press)))

(defun wl-trace-check-file (file rules)
  "Check FILE (a WAYLAND_DEBUG=client trace) against RULES (a list of names).
Returns a list of strings \"RULE line N: what\", empty when the trace is clean."
  (let ((on (lambda (r) (member r rules)))
        (out nil)
        (n 0)
        (counts (make-hash-table :test 'equal))
        (sample nil)
        (last-time 0.0)
        ;; object tables, by id
        (xdg-wl (make-hash-table))        ; xdg_surface id -> wl_surface id
        (wl-xdg (make-hash-table))        ; wl_surface id -> xdg_surface id
        (top-xdg (make-hash-table))       ; xdg_toplevel id -> xdg_surface id
        (popup-xdg (make-hash-table))     ; xdg_popup id -> xdg_surface id
        (sub-child (make-hash-table))     ; wl_subsurface id -> wl_surface id (the child)
        (sub-parent (make-hash-table))    ; wl_subsurface id -> wl_surface id (the parent)
        (parent-of (make-hash-table))     ; wl_surface id (a child) -> its parent's wl_surface id
        (sub-of (make-hash-table))        ; wl_surface id (a child) -> its wl_subsurface id
        ;; acks
        (cfg-size (make-hash-table))      ; xdg_surface id -> the newest xdg_toplevel.configure size
        (serial-size (make-hash-table))   ; serial -> size
        (acked-size (make-hash-table))    ; xdg_surface id -> the previous acked non-zero size
        (acked-once (make-hash-table))    ; xdg_surface id -> t
        (need-buffer (make-hash-table))   ; wl_surface id -> line of the ack that wants a buffer
        (attached (make-hash-table))      ; wl_surface id -> :buffer, :null or nil since its last commit
        ;; popups and drags
        (popup-parent (make-hash-table))  ; xdg_popup id -> the xdg_surface id it was made on, or nil
        (popup-live (make-hash-table))    ; xdg_popup id -> t until it is destroyed
        (grabbing nil)                    ; the xdg_popup ids that grab, topmost first
        (press (make-hash-table))         ; serial -> (KIND . KEY) of a press the client saw
        (released (make-hash-table))      ; serial -> t once its button, key or touch point was let go
        (offer-state (make-hash-table))   ; wl_data_offer id -> :entered or :acted
        (offer-dead (make-hash-table))    ; wl_data_offer id -> t after finish or destroy
        ;; deadline rules: each entry is a list, see the rule
        (sub-pending nil)                 ; (DEADLINE PARENT LINE WHAT)
        (popup-pending nil)               ; (DEADLINE XDG WL LINE ACKED)
        (sync-pending nil))               ; (DEADLINE CHAIN LINE CHILD), CHAIN the ancestors still to commit
    (cl-flet ((bad (rule line fmt &rest a)
                (when (funcall on rule)
                  (push (format "%s line %d: %s" rule line (apply #'format fmt a)) out))))
      (with-temp-buffer
        (insert-file-contents file)
        (goto-char (point-min))
        (while (not (eobp))
          (cl-incf n)
          (let ((text (buffer-substring-no-properties (line-beginning-position) (line-end-position))))
            (if (not (string-match wl-trace-check--line-re text))
                (progn
                  (when (< (length sample) 6) (push (truncate-string-to-width text 160) sample))
                  (when (string-match wl-trace-check--error-re text)
                    (bad "errors" n "%s" (string-trim text))))
              (let* ((time (wl-trace-check--ms (match-string 1 text)))
                     (request (match-string 2 text))
                     (iface (match-string 3 text))
                     (id (string-to-number (match-string 4 text)))
                     (msg (match-string 5 text))
                     (args (match-string 6 text))
                     (refs (wl-trace-check--refs args)))
                (setq last-time time)
                (cl-incf (gethash "trace lines" counts 0))
                (cl-incf (gethash (format "%s.%s" iface msg) counts 0))
                ;; deadlines that ran out before this line
                (dolist (p (cl-remove-if-not (lambda (p) (< (car p) time)) sub-pending))
                  (bad "subsurface" (nth 2 p) "%s: parent %d did not commit within %d ms" (nth 3 p) (nth 1 p)
                       (round wl-trace-check-window)))
                (setq sub-pending (cl-remove-if (lambda (p) (< (car p) time)) sub-pending))
                (dolist (p (cl-remove-if-not (lambda (p) (< (car p) time)) popup-pending))
                  (bad "popup" (nth 3 p) "repositioned: %s within %d ms" (if (nth 4 p) "no commit" "no ack_configure")
                       (round wl-trace-check-window)))
                (setq popup-pending (cl-remove-if (lambda (p) (< (car p) time)) popup-pending))
                (dolist (p (cl-remove-if-not (lambda (p) (< (car p) time)) sync-pending))
                  (bad "sync" (nth 2 p) "commit of %d: ancestors %S did not commit within %d ms" (nth 3 p) (nth 1 p)
                       (round wl-trace-check-window)))
                (setq sync-pending (cl-remove-if (lambda (p) (< (car p) time)) sync-pending))
                (cond
                 ;; ---- errors and object lifetime
                 ((and (equal iface "wl_display") (equal msg "error"))
                  (bad "errors" n "wl_display.error(%s)" args))
                 ((and (equal iface "wl_display") (equal msg "delete_id"))
                  (let ((gone (string-to-number args)))
                    (let ((child (gethash gone sub-child)) (w (gethash gone xdg-wl)))
                      (when child (remhash child parent-of) (remhash child sub-of))
                      (when w (remhash w wl-xdg)))
                    (setq grabbing (delq gone grabbing))
                    (dolist (tab (list xdg-wl wl-xdg top-xdg popup-xdg sub-child sub-parent parent-of sub-of
                                       cfg-size acked-size acked-once need-buffer attached
                                       popup-parent popup-live offer-state offer-dead))
                      (remhash gone tab))))
                 ;; ---- object graph
                 ((and request (equal msg "get_xdg_surface"))
                  (let ((x (cdr (nth 0 refs))) (w (cdr (nth 1 refs))))
                    (puthash x w xdg-wl)
                    (puthash w x wl-xdg)))
                 ((and request (equal msg "get_toplevel"))
                  (puthash (cdr (nth 0 refs)) id top-xdg))
                 ((and request (equal msg "get_popup"))
                  (let ((parent (nth 1 refs)))
                    (puthash (cdr (nth 0 refs)) id popup-xdg)
                    (puthash (cdr (nth 0 refs)) (and (equal (car parent) "xdg_surface") (cdr parent)) popup-parent)
                    (puthash (cdr (nth 0 refs)) t popup-live)))
                 ((and request (equal msg "get_subsurface"))
                  (let ((s (cdr (nth 0 refs))) (child (cdr (nth 1 refs))) (parent (cdr (nth 2 refs))))
                    (puthash s child sub-child)
                    (puthash s parent sub-parent)
                    (puthash child parent parent-of)
                    (puthash child s sub-of)))
                 ((and request (equal iface "wl_subsurface") (equal msg "destroy"))
                  (let ((child (gethash id sub-child)))
                    (setq sub-pending (cl-remove-if (lambda (p) (eq (nth 4 p) id)) sub-pending))
                    (when child
                      (remhash child parent-of)
                      (remhash child sub-of))))
                 ;; ---- xdg configure and ack
                 ((and (not request) (equal iface "xdg_toplevel") (equal msg "configure"))
                  (let ((x (gethash id top-xdg)))
                    (when (and x (string-match "\\`\\(-?[0-9]+\\), \\(-?[0-9]+\\)" args))
                      (puthash x (cons (string-to-number (match-string 1 args))
                                       (string-to-number (match-string 2 args)))
                               cfg-size))))
                 ((and (not request) (equal iface "xdg_surface") (equal msg "configure"))
                  (puthash (string-to-number args) (gethash id cfg-size) serial-size))
                 ((and (not request) (equal iface "xdg_popup") (equal msg "repositioned"))
                  (let ((x (gethash id popup-xdg)))
                    (when x
                      (push (list (+ time wl-trace-check-window) x (gethash x xdg-wl) n nil) popup-pending))))
                 ((and request (equal iface "xdg_surface") (equal msg "ack_configure"))
                  (let* ((size (gethash (string-to-number args) serial-size))
                         (w (gethash id xdg-wl)))
                    (puthash id t acked-once)
                    (when (and size (> (car size) 0) (> (cdr size) 0))
                      (when (and w (not (equal size (gethash id acked-size))))
                        (puthash w n need-buffer))
                      (puthash id size acked-size))
                    (dolist (p popup-pending)
                      (when (eq (nth 1 p) id) (setf (nth 4 p) t)))))
                 ;; ---- wl_surface
                 ((and request (equal iface "wl_surface") (equal msg "attach"))
                  (let ((buf (if (string-prefix-p "nil" args) :null :buffer)))
                    (puthash id buf attached)
                    (when (and (eq buf :buffer) (gethash id wl-xdg)
                               (not (gethash (gethash id wl-xdg) acked-once)))
                      (bad "unconfigured" n "buffer attached to wl_surface %d before its first ack_configure" id))))
                 ((and request (equal iface "wl_surface") (equal msg "commit"))
                  (let ((line (gethash id need-buffer)))
                    (when line
                      (unless (eq (gethash id attached) :buffer)
                        (bad "acks" n "commit of wl_surface %d after the ack_configure of line %d (a new size) has no buffer"
                             id line))
                      (remhash id need-buffer)))
                  (remhash id attached)
                  ;; subsurface: the parent committed
                  (setq sub-pending (cl-remove-if (lambda (p) (eq (nth 1 p) id)) sub-pending))
                  ;; popup: acked, then committed
                  (setq popup-pending (cl-remove-if (lambda (p) (and (eq (nth 2 p) id) (nth 4 p))) popup-pending))
                  ;; sync: ancestors in order
                  (dolist (p sync-pending)
                    (cond ((eq (car (nth 1 p)) id) (setf (nth 1 p) (cdr (nth 1 p))))
                          ((memq id (nth 1 p))
                           (bad "sync" n "wl_surface %d committed before %d (a descendant commit of line %d needs deepest first)"
                                id (car (nth 1 p)) (nth 2 p))
                           (setf (nth 1 p) (cdr (memq id (nth 1 p)))))))
                  (setq sync-pending (cl-remove-if (lambda (p) (null (nth 1 p))) sync-pending))
                  (when (gethash id parent-of)
                    (let (chain (s id))
                      (while (setq s (gethash s parent-of)) (push s chain))
                      (push (list (+ time wl-trace-check-window) (nreverse chain) n id) sync-pending))))
                 ;; ---- subsurface state
                 ((and request (equal iface "wl_subsurface") (equal msg "set_desync"))
                  (bad "sync" n "wl_subsurface %d set_desync" id))
                 ((and request (equal iface "wl_subsurface")
                       (member msg '("set_position" "place_above" "place_below")))
                  (let ((parent (gethash id sub-parent)))
                    (when parent
                      (push (list (+ time wl-trace-check-window) parent n (format "%s(%s)" msg args) id)
                            sub-pending))))
                 ;; ---- the presses the client saw (button, key, touch down) and what let them go
                 ((and (not request) (equal iface "wl_pointer") (equal msg "button")
                       (string-match "\\`\\([0-9]+\\), [0-9]+, \\([0-9]+\\), \\([0-9]+\\)" args))
                  (wl-trace-check--press press released (string-to-number (match-string 1 args))
                                         (format "b%s" (match-string 2 args)) (equal (match-string 3 args) "1") 'button))
                 ((and (not request) (equal iface "wl_keyboard") (equal msg "key")
                       (string-match "\\`\\([0-9]+\\), [0-9]+, \\([0-9]+\\), \\([0-9]+\\)" args))
                  (wl-trace-check--press press released (string-to-number (match-string 1 args))
                                         (format "k%s" (match-string 2 args)) (equal (match-string 3 args) "1") 'key))
                 ((and (not request) (equal iface "wl_touch") (equal msg "down")
                       (string-match "\\`\\([0-9]+\\), [0-9]+, [a-z_0-9]+[@#][0-9]+, \\(-?[0-9]+\\)" args))
                  (wl-trace-check--press press released (string-to-number (match-string 1 args))
                                         (format "t%s" (match-string 2 args)) t 'touch))
                 ((and (not request) (equal iface "wl_touch") (equal msg "up")
                       (string-match "\\`[0-9]+, [0-9]+, \\(-?[0-9]+\\)" args))
                  (wl-trace-check--press press released nil (format "t%s" (match-string 1 args)) nil 'touch))
                 ;; ---- grab and popup order
                 ((and request (equal iface "xdg_popup") (equal msg "grab")
                       (string-match "[a-z_0-9]+[@#][0-9]+, \\([0-9]+\\)\\'" args))
                  (let* ((serial (string-to-number (match-string 1 args)))
                         (parent (gethash id popup-parent))
                         (top (car grabbing)))
                    (cond
                     ((not (gethash serial press))
                      (bad "grab" n "xdg_popup %d grabs with serial %d, which no press the client saw carried" id serial))
                     ((and parent
                           (not (cl-loop for x being the hash-values of top-xdg thereis (eq x parent)))
                           (not (and top (eq parent (gethash top popup-xdg)))))
                      (bad "grab" n "xdg_popup %d grabs on xdg_surface %d, neither a toplevel nor the topmost grabbing popup"
                           id parent)))
                    (push id grabbing)))
                 ((and request (equal iface "xdg_popup") (equal msg "destroy"))
                  (let* ((x (gethash id popup-xdg))
                         (child (and x (cl-loop for c being the hash-keys of popup-parent
                                                thereis (and (not (eq c id)) (gethash c popup-live)
                                                             (eq (gethash c popup-parent) x) c)))))
                    (when child
                      (bad "popup-order" n "xdg_popup %d destroyed before its child popup %d" id child)))
                  (remhash id popup-live)
                  (setq grabbing (delq id grabbing)))
                 ;; ---- drag and drop
                 ((and (not request) (equal iface "wl_data_device") (equal msg "enter"))
                  (let ((offer (cl-find "wl_data_offer" refs :key #'car :test #'equal)))
                    (when offer (puthash (cdr offer) :entered offer-state))))
                 ((and request (equal iface "wl_data_offer") (equal msg "set_actions")
                       (string-match "\\`\\([0-9]+\\)" args))
                  (puthash id (if (equal (match-string 1 args) "0") :entered :acted) offer-state))
                 ((and request (equal iface "wl_data_offer") (equal msg "finish"))
                  (unless (eq (gethash id offer-state) :acted)
                    (bad "dnd" n "wl_data_offer %d finish without a non-zero set_actions since its enter" id))
                  (puthash id t offer-dead))
                 ((and request (equal iface "wl_data_offer") (equal msg "receive"))
                  (when (gethash id offer-dead)
                    (bad "dnd" n "wl_data_offer %d receive after finish or destroy" id)))
                 ((and request (equal iface "wl_data_offer") (equal msg "destroy"))
                  (puthash id t offer-dead))
                 ((and request (equal iface "wl_data_device") (equal msg "start_drag")
                       (string-match ", \\([0-9]+\\)\\'" args))
                  (let* ((serial (string-to-number (match-string 1 args)))
                         (p (gethash serial press)))
                    (cond ((or (null p) (eq (car p) 'key))
                           (bad "dnd" n "start_drag with serial %d, which is no press (button or touch down) the client saw" serial))
                          ((gethash serial released)
                           (bad "dnd" n "start_drag with serial %d: its press was released" serial))))))))
            (forward-line 1))))
      ;; the trace ended: only what had its whole window inside the trace is a violation
      (dolist (p sub-pending)
        (when (< (car p) last-time)
          (bad "subsurface" (nth 2 p) "%s: parent %d never committed" (nth 3 p) (nth 1 p))))
      (dolist (p popup-pending)
        (when (< (car p) last-time)
          (bad "popup" (nth 3 p) "repositioned: %s" (if (nth 4 p) "no commit" "no ack_configure"))))
      (dolist (p sync-pending)
        (when (< (car p) last-time)
          (bad "sync" (nth 2 p) "commit of %d: ancestors %S never committed" (nth 3 p) (nth 1 p)))))
    (setq wl-trace-check-sample (nreverse sample))
    (setq wl-trace-check-counts
          (mapcar (lambda (k) (cons k (gethash k counts 0)))
                  '("trace lines" "wl_surface.commit" "xdg_surface.ack_configure" "wl_subsurface.set_position"
                    "wl_subsurface.place_above" "wl_subsurface.place_below" "xdg_popup.repositioned"
                    "xdg_popup.grab" "xdg_popup.destroy" "wl_data_offer.finish" "wl_data_device.start_drag")))
    (nreverse out)))

(defun wl-trace-check-batch ()
  "Batch entry: TRACE RULES from the command line; print the violations, exit 1 when there are any."
  (let* ((file (pop command-line-args-left))
         (rules (split-string (or (pop command-line-args-left) "") "," t))
         (unknown (cl-remove-if (lambda (r) (member r wl-trace-check-rules)) rules)))
    (when (or (null file) (null rules) unknown (not (file-readable-p file)))
      (message "wl-trace-check: usage: TRACE RULES (%s)%s" (string-join wl-trace-check-rules ",")
               (if unknown (format "; unknown: %s" unknown) ""))
      (kill-emacs 2))
    (let ((bad (wl-trace-check-file file rules)))
      ;; a trace with no protocol line in it would pass every rule: say so and fail
      (when (zerop (cdr (assoc "trace lines" wl-trace-check-counts)))
        (princ (format "wl-trace-check: no protocol line in %s; it starts:\n  %s\n" (file-name-nondirectory file)
                       (string-join wl-trace-check-sample "\n  ")))
        (kill-emacs 1))
      (dolist (b bad) (princ (format "wl-trace-check: %s\n" b)))
      (princ (format "wl-trace-check: saw %s\n"
                     (mapconcat (lambda (c) (format "%s %d" (car c) (cdr c))) wl-trace-check-counts ", ")))
      (princ (format "wl-trace-check: %s: %d violation(s) in %s\n" (string-join rules ",") (length bad)
                     (file-name-nondirectory file)))
      (kill-emacs (if bad 1 0)))))

(provide 'wl-trace-check)
;;; wl-trace-check.el ends here
