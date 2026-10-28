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

(defconst wl-trace-check-test--grab
  '((1000.0 "> xdg_wm_base@10.get_xdg_surface(new id xdg_surface@20, wl_surface@5)")
    (1000.1 "> xdg_surface@20.get_toplevel(new id xdg_toplevel@21)")
    (1000.2 "> xdg_surface@22.get_popup(new id xdg_popup@23, xdg_surface@20, xdg_positioner@24)")
    (1000.3 "> xdg_surface@25.get_popup(new id xdg_popup@26, xdg_surface@22, xdg_positioner@24)")
    (1000.4 "> xdg_surface@27.get_popup(new id xdg_popup@28, xdg_surface@22, xdg_positioner@24)")
    (1001.0 "wl_pointer@8.button(9, 1000, 272, 1)"))
  "Toplevel xdg_surface 20, popups 23 (on it), 26 and 28 (both on popup 23), and a press with serial 9.")

(ert-deftest wl-trace-check/grab-rule ()
  ;; a grab with a seen press, on a toplevel, then a child of the grabbing popup: clean
  (should-not (wl-trace-check-test--run
               '("grab")
               wl-trace-check-test--grab
               '(1002.0 "> xdg_popup@23.grab(wl_seat@6, 9)")
               '(1002.1 "> xdg_popup@26.grab(wl_seat@6, 9)")))
  ;; a serial no press carried
  (let ((v (wl-trace-check-test--run
            '("grab")
            wl-trace-check-test--grab
            '(1002.0 "> xdg_popup@23.grab(wl_seat@6, 77)"))))
    (should (= (length v) 1))
    (should (string-match-p "\\`grab line [0-9]+: xdg_popup 23 grabs with serial 77" (car v))))
  ;; a key press is a press too; a grab under a popup that does not grab is not under the topmost grabbing popup
  (should-not (wl-trace-check-test--run
               '("grab")
               wl-trace-check-test--grab
               '(1001.5 "wl_keyboard@7.key(11, 1000, 30, 1)")
               '(1002.0 "> xdg_popup@23.grab(wl_seat@6, 11)")))
  (let ((v (wl-trace-check-test--run
            '("grab")
            wl-trace-check-test--grab
            '(1002.0 "> xdg_popup@26.grab(wl_seat@6, 9)"))))
    (should (= (length v) 1))
    (should (string-match-p "neither a toplevel nor the topmost grabbing popup" (car v))))
  ;; the second child of popup 23 is not under the topmost grabbing popup (26) either
  (let ((v (wl-trace-check-test--run
            '("grab")
            wl-trace-check-test--grab
            '(1002.0 "> xdg_popup@23.grab(wl_seat@6, 9)")
            '(1002.1 "> xdg_popup@26.grab(wl_seat@6, 9)")
            '(1002.2 "> xdg_popup@28.grab(wl_seat@6, 9)"))))
    (should (= (length v) 1))
    (should (string-match-p "xdg_popup 28" (car v))))
  ;; a grabbing popup that was destroyed no longer counts
  (should-not (wl-trace-check-test--run
               '("grab")
               wl-trace-check-test--grab
               '(1002.0 "> xdg_popup@23.grab(wl_seat@6, 9)")
               '(1002.1 "> xdg_popup@23.destroy()")
               '(1002.2 "> xdg_surface@29.get_popup(new id xdg_popup@30, xdg_surface@20, xdg_positioner@24)")
               '(1002.3 "> xdg_popup@30.grab(wl_seat@6, 9)"))))

(ert-deftest wl-trace-check/popup-order-rule ()
  (should-not (wl-trace-check-test--run
               '("popup-order")
               wl-trace-check-test--grab
               '(1002.0 "> xdg_popup@26.destroy()")
               '(1002.1 "> xdg_popup@28.destroy()")
               '(1002.2 "> xdg_popup@23.destroy()")))
  (let ((v (wl-trace-check-test--run
            '("popup-order")
            wl-trace-check-test--grab
            '(1002.0 "> xdg_popup@23.destroy()")
            '(1002.1 "> xdg_popup@26.destroy()"))))
    (should (= (length v) 1))
    (should (string-match-p "\\`popup-order line [0-9]+: xdg_popup 23 destroyed before its child popup" (car v))))
  ;; an unselected rule reports nothing
  (should-not (wl-trace-check-test--run
               '("grab")
               wl-trace-check-test--grab
               '(1002.0 "> xdg_popup@23.destroy()"))))

(defconst wl-trace-check-test--dnd
  '((1000.0 "wl_pointer@8.button(9, 1000, 272, 1)")
    (1001.0 "wl_data_device@12.enter(14, wl_surface@5, 10.0, 12.0, wl_data_offer@40)"))
  "A press (serial 9) and a drag entering with offer 40.")

(ert-deftest wl-trace-check/dnd-rule ()
  ;; set_actions before the finish
  (should-not (wl-trace-check-test--run
               '("dnd")
               wl-trace-check-test--dnd
               '(1002.0 "> wl_data_offer@40.set_actions(1, 1)")
               '(1003.0 "> wl_data_offer@40.receive(\"text/plain\", fd 7)")
               '(1004.0 "> wl_data_offer@40.finish()")
               '(1004.1 "> wl_data_offer@40.destroy()")))
  ;; a finish with no set_actions, or with a zero one
  (let ((v (wl-trace-check-test--run
            '("dnd")
            wl-trace-check-test--dnd
            '(1004.0 "> wl_data_offer@40.finish()"))))
    (should (= (length v) 1))
    (should (string-match-p "\\`dnd line [0-9]+: wl_data_offer 40 finish without a non-zero set_actions" (car v))))
  (should (wl-trace-check-test--run
           '("dnd")
           wl-trace-check-test--dnd
           '(1002.0 "> wl_data_offer@40.set_actions(0, 0)")
           '(1004.0 "> wl_data_offer@40.finish()")))
  ;; a new enter wants new actions
  (should (wl-trace-check-test--run
           '("dnd")
           wl-trace-check-test--dnd
           '(1002.0 "> wl_data_offer@40.set_actions(1, 1)")
           '(1003.0 "wl_data_device@12.enter(15, wl_surface@5, 1.0, 2.0, wl_data_offer@40)")
           '(1004.0 "> wl_data_offer@40.finish()")))
  ;; no receive after finish or destroy
  (let ((v (wl-trace-check-test--run
            '("dnd")
            wl-trace-check-test--dnd
            '(1002.0 "> wl_data_offer@40.set_actions(1, 1)")
            '(1004.0 "> wl_data_offer@40.finish()")
            '(1005.0 "> wl_data_offer@40.receive(\"text/plain\", fd 7)"))))
    (should (= (length v) 1))
    (should (string-match-p "receive after finish or destroy" (car v))))
  (should (wl-trace-check-test--run
           '("dnd")
           wl-trace-check-test--dnd
           '(1004.0 "> wl_data_offer@40.destroy()")
           '(1005.0 "> wl_data_offer@40.receive(\"text/plain\", fd 7)")))
  ;; start_drag: the press serial, before its release
  (should-not (wl-trace-check-test--run
               '("dnd")
               wl-trace-check-test--dnd
               '(1002.0 "> wl_data_device@12.start_drag(wl_data_source@50, wl_surface@5, nil, 9)")))
  (let ((v (wl-trace-check-test--run
            '("dnd")
            wl-trace-check-test--dnd
            '(1002.0 "wl_pointer@8.button(10, 1100, 272, 0)")
            '(1003.0 "> wl_data_device@12.start_drag(wl_data_source@50, wl_surface@5, nil, 9)"))))
    (should (= (length v) 1))
    (should (string-match-p "start_drag with serial 9: its press was released" (car v))))
  (let ((v (wl-trace-check-test--run
            '("dnd")
            wl-trace-check-test--dnd
            '(1003.0 "> wl_data_device@12.start_drag(wl_data_source@50, wl_surface@5, nil, 31)"))))
    (should (= (length v) 1))
    (should (string-match-p "start_drag with serial 31, which is no press" (car v))))
  ;; a touch down counts, until its up
  (should-not (wl-trace-check-test--run
               '("dnd")
               '(1000.0 "wl_touch@9.down(21, 1000, wl_surface@5, 2, 5.0, 6.0)")
               '(1001.0 "> wl_data_device@12.start_drag(wl_data_source@50, wl_surface@5, nil, 21)")))
  (should (wl-trace-check-test--run
           '("dnd")
           '(1000.0 "wl_touch@9.down(21, 1000, wl_surface@5, 2, 5.0, 6.0)")
           '(1000.5 "wl_touch@9.up(22, 1010, 2)")
           '(1001.0 "> wl_data_device@12.start_drag(wl_data_source@50, wl_surface@5, nil, 21)"))))

;;; wl-trace-check-gui-tests.el ends here
