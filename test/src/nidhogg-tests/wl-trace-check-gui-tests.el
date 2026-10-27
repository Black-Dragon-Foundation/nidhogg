;;; wl-trace-check-gui-tests.el --- the protocol-trace checker, on traces written by hand  -*- lexical-binding: t -*-

;; Each rule of nidhogg/test/wl-trace-check.el is run on a trace that breaks it (it must report it) and on one that
;; does not (it must stay silent).  The rows need no frame and no compositor; they run with `--select wl-trace-check/'.

(require 'ert)
(require 'seq)

(load (expand-file-name "../../../nidhogg/test/wl-trace-check.el"
                        (file-name-directory (or load-file-name buffer-file-name)))
      nil t)

(defun wl-trace-check-test--run (rules &rest lines)
  "The violations of RULES in a trace of LINES, each (TIME TEXT) or a list of those: a request when TEXT starts with \"> \"."
  (let ((file (make-temp-file "wl-trace" nil ".txt")))
    (unwind-protect
        (progn
          (with-temp-file file
            (insert "some test output that is no trace line\n")
            (dolist (l (seq-mapcat (lambda (l) (if (numberp (car l)) (list l) l)) lines))
              (let* ((text (cadr l)) (req (string-prefix-p "> " text)))
                (insert (format "[%.3f] {Default Queue} %s%s\n" (car l) (if req " -> " "")
                                (if req (substring text 2) text))))))
          (wl-trace-check-file file rules))
      (delete-file file))))

(defconst wl-trace-check-test--toplevel
  '((1000.0 "> xdg_wm_base@10.get_xdg_surface(new id xdg_surface@20, wl_surface@5)")
    (1000.1 "> xdg_surface@20.get_toplevel(new id xdg_toplevel@21)")
    (1001.0 "xdg_toplevel@21.configure(300, 200, array[0])")
    (1001.1 "xdg_surface@20.configure(7)"))
  "A toplevel that has been configured to 300 x 200 (serial 7).")

(defun wl-trace-check-test--toplevel (&rest more)
  (append wl-trace-check-test--toplevel more))

(ert-deftest wl-trace-check/acks-a-new-size-needs-a-buffer ()
  (should-not (wl-trace-check-test--run
               '("acks" "unconfigured")
               (wl-trace-check-test--toplevel
                '(1002.0 "> xdg_surface@20.ack_configure(7)")
                '(1002.1 "> wl_surface@5.attach(wl_buffer@30, 0, 0)")
                '(1002.2 "> wl_surface@5.commit()"))))
  (let ((v (wl-trace-check-test--run
            '("acks")
            (wl-trace-check-test--toplevel
             '(1002.0 "> xdg_surface@20.ack_configure(7)")
             '(1002.2 "> wl_surface@5.commit()")))))
    (should (= (length v) 1))
    (should (string-match-p "\\`acks line [0-9]+: commit of wl_surface 5" (car v)))))

(ert-deftest wl-trace-check/acks-same-size-needs-none ()
  (should-not (wl-trace-check-test--run
               '("acks")
               (wl-trace-check-test--toplevel
                '(1002.0 "> xdg_surface@20.ack_configure(7)")
                '(1002.1 "> wl_surface@5.attach(wl_buffer@30, 0, 0)")
                '(1002.2 "> wl_surface@5.commit()")
                '(1003.0 "xdg_toplevel@21.configure(300, 200, array[0])")
                '(1003.1 "xdg_surface@20.configure(8)")
                '(1003.2 "> xdg_surface@20.ack_configure(8)")
                '(1003.3 "> wl_surface@5.commit()")))))

(ert-deftest wl-trace-check/unconfigured-attach-before-the-first-ack ()
  (let ((v (wl-trace-check-test--run
            '("unconfigured")
            (wl-trace-check-test--toplevel
             '(1002.0 "> wl_surface@5.attach(wl_buffer@30, 0, 0)")
             '(1002.1 "> xdg_surface@20.ack_configure(7)")))))
    (should (= (length v) 1))
    (should (string-match-p "before its first ack_configure" (car v))))
  (should-not (wl-trace-check-test--run
               '("unconfigured")
               (wl-trace-check-test--toplevel
                '(1002.0 "> wl_surface@5.attach(nil, 0, 0)")
                '(1002.1 "> xdg_surface@20.ack_configure(7)")
                '(1002.2 "> wl_surface@5.attach(wl_buffer@30, 0, 0)")))))

(defconst wl-trace-check-test--sub
  '((1000.0 "> wl_subcompositor@11.get_subsurface(new id wl_subsurface@40, wl_surface@6, wl_surface@5)"))
  "Surface 6 is a subsurface (40) of surface 5.")

(ert-deftest wl-trace-check/subsurface-state-needs-a-parent-commit ()
  (should-not (wl-trace-check-test--run
               '("subsurface")
               (car wl-trace-check-test--sub)
               '(1010.0 "> wl_subsurface@40.set_position(10, 20)")
               '(1050.0 "> wl_surface@5.commit()")
               '(1300.0 "> wl_surface@7.commit()")))
  (let ((v (wl-trace-check-test--run
            '("subsurface")
            (car wl-trace-check-test--sub)
            '(1010.0 "> wl_subsurface@40.place_above(wl_surface@8)")
            '(1300.0 "> wl_surface@7.commit()"))))
    (should (= (length v) 1))
    (should (string-match-p "parent 5 did not commit" (car v)))))

(defconst wl-trace-check-test--popup
  '((1000.0 "> xdg_wm_base@10.get_xdg_surface(new id xdg_surface@22, wl_surface@9)")
    (1000.1 "> xdg_surface@22.get_popup(new id xdg_popup@23, xdg_surface@20, xdg_positioner@24)")))

(ert-deftest wl-trace-check/popup-reposition-needs-an-ack-and-a-commit ()
  (should-not (wl-trace-check-test--run
               '("popup")
               wl-trace-check-test--popup
               '(1010.0 "xdg_popup@23.repositioned(3)")
               '(1010.1 "xdg_surface@22.configure(9)")
               '(1020.0 "> xdg_surface@22.ack_configure(9)")
               '(1020.1 "> wl_surface@9.commit()")
               '(1300.0 "> wl_surface@7.commit()")))
  (let ((v (wl-trace-check-test--run
            '("popup")
            wl-trace-check-test--popup
            '(1010.0 "xdg_popup@23.repositioned(3)")
            '(1010.1 "xdg_surface@22.configure(9)")
            '(1300.0 "> wl_surface@7.commit()"))))
    (should (= (length v) 1))
    (should (string-match-p "no ack_configure" (car v))))
  (let ((v (wl-trace-check-test--run
            '("popup")
            wl-trace-check-test--popup
            '(1010.0 "xdg_popup@23.repositioned(3)")
            '(1020.0 "> xdg_surface@22.ack_configure(9)")
            '(1300.0 "> wl_surface@7.commit()"))))
    (should (= (length v) 1))
    (should (string-match-p "no commit" (car v)))))

(defconst wl-trace-check-test--nested
  '((1000.0 "> wl_subcompositor@11.get_subsurface(new id wl_subsurface@40, wl_surface@6, wl_surface@5)")
    (1000.1 "> wl_subcompositor@11.get_subsurface(new id wl_subsurface@41, wl_surface@8, wl_surface@6)"))
  "Surface 8 is a child of 6, which is a child of the root 5.")

(ert-deftest wl-trace-check/sync-ancestors-commit-deepest-first ()
  (should-not (wl-trace-check-test--run
               '("sync")
               wl-trace-check-test--nested
               '(1010.0 "> wl_surface@8.commit()")
               '(1010.1 "> wl_surface@6.commit()")
               '(1010.2 "> wl_surface@5.commit()")
               '(1300.0 "> wl_surface@7.commit()")))
  (let ((v (wl-trace-check-test--run
            '("sync")
            wl-trace-check-test--nested
            '(1010.0 "> wl_surface@8.commit()")
            '(1010.1 "> wl_surface@5.commit()")
            '(1010.2 "> wl_surface@6.commit()")
            '(1300.0 "> wl_surface@7.commit()"))))
    (should (seq-find (lambda (s) (string-match-p "committed before 6" s)) v)))
  (let ((v (wl-trace-check-test--run
            '("sync")
            wl-trace-check-test--nested
            '(1010.0 "> wl_surface@8.commit()")
            '(1300.0 "> wl_surface@7.commit()"))))
    (should (seq-find (lambda (s) (string-match-p "did not commit within" s)) v)))
  (should (wl-trace-check-test--run
           '("sync")
           wl-trace-check-test--nested
           '(1010.0 "> wl_subsurface@41.set_desync()"))))

(ert-deftest wl-trace-check/errors-and-unselected-rules ()
  (let ((v (wl-trace-check-test--run
            '("errors")
            '(1000.0 "wl_display@1.error(wl_surface@5, 1, \"bad\")"))))
    (should (= (length v) 1)))
  ;; a rule that was not asked for reports nothing
  (should-not (wl-trace-check-test--run
               '("acks")
               '(1000.0 "wl_display@1.error(wl_surface@5, 1, \"bad\")")))
  ;; an id deleted by the server is free again: a new object of the same id starts clean
  (should-not (wl-trace-check-test--run
               '("unconfigured")
               (wl-trace-check-test--toplevel
                '(1002.0 "> xdg_surface@20.ack_configure(7)")
                '(1002.1 "> wl_surface@5.attach(wl_buffer@30, 0, 0)")
                '(1003.0 "wl_display@1.delete_id(20)")
                '(1003.1 "> wl_surface@5.attach(wl_buffer@30, 0, 0)")))))

(ert-deftest wl-trace-check/libwayland-wall-clock-format ()
  "The format libwayland 1.24 prints: a wall-clock time, `iface#id', the queue name on events only."
  (let ((file (make-temp-file "wl-trace" nil ".txt")))
    (unwind-protect
        (progn
          (with-temp-file file
            (insert "[17:53:42.056559]  -> wl_subcompositor#11.get_subsurface(new id wl_subsurface#40, wl_surface#6, wl_surface#5)\n"
                    "[17:53:42.056600]  -> wl_subsurface#40.set_position(10, 20)\n"
                    "[17:53:42.300000] {Default Queue} wl_callback#7.done(1234)\n"
                    "[17:53:42.300100]  -> wl_surface#7.commit()\n"))
          (let ((v (wl-trace-check-file file '("subsurface"))))
            (should (= (length v) 1))
            (should (string-match-p "parent 5 did not commit" (car v))))
          (should (= (cdr (assoc "trace lines" wl-trace-check-counts)) 4)))
      (delete-file file))))

;;; wl-trace-check-gui-tests.el ends here
