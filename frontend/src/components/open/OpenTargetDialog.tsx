import { FormEvent, useEffect, useRef, useState } from "react";
import jsQR from "jsqr";
import type { Worker } from "tesseract.js";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router-dom";
import { api, getErrorMessage } from "../../api/client";
import Modal from "../common/Modal";
import styles from "./OpenTargetDialog.module.css";

interface Props {
  open: boolean;
  onClose: () => void;
  onBeforeNavigate?: () => void;
}

const TARGET_PATTERN =
  /(?:https?:\/\/[^\s]+|at:\/\/[^\s]+|did:plc:[a-z0-9]+|@[^\s]+)/i;

export default function OpenTargetDialog({
  open,
  onClose,
  onBeforeNavigate,
}: Props) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const [target, setTarget] = useState("");
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  const [scanning, setScanning] = useState(false);
  const videoRef = useRef<HTMLVideoElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const streamRef = useRef<MediaStream | null>(null);
  const workerRef = useRef<Worker | null>(null);
  const recognizingRef = useRef(false);

  function stopScanning() {
    streamRef.current?.getTracks().forEach((track) => track.stop());
    streamRef.current = null;
    void workerRef.current?.terminate();
    workerRef.current = null;
    recognizingRef.current = false;
    setScanning(false);
  }

  useEffect(() => {
    if (!open) {
      stopScanning();
      setError("");
    }
    return () => stopScanning();
  }, [open]);

  useEffect(() => {
    if (!scanning) return;
    let frameId = 0;
    let lastOcrAt = 0;

    const acceptRecognized = (text: string) => {
      const matched = text.match(TARGET_PATTERN)?.[0]?.replace(/[),.;]+$/, "");
      if (!matched) return false;
      setTarget(matched);
      stopScanning();
      return true;
    };

    const scanFrame = async (now: number) => {
      const video = videoRef.current;
      const canvas = canvasRef.current;
      if (
        !video ||
        !canvas ||
        video.readyState < HTMLMediaElement.HAVE_CURRENT_DATA
      ) {
        frameId = requestAnimationFrame(scanFrame);
        return;
      }
      const width = video.videoWidth;
      const height = video.videoHeight;
      if (!width || !height) {
        frameId = requestAnimationFrame(scanFrame);
        return;
      }
      canvas.width = width;
      canvas.height = height;
      const context = canvas.getContext("2d", { willReadFrequently: true });
      if (!context) return;
      context.drawImage(video, 0, 0, width, height);
      const image = context.getImageData(0, 0, width, height);
      const qr = jsQR(image.data, width, height);
      if (qr && acceptRecognized(qr.data)) return;

      if (now - lastOcrAt >= 2000 && !recognizingRef.current) {
        lastOcrAt = now;
        recognizingRef.current = true;
        try {
          if (!workerRef.current) {
            const { createWorker } = await import("tesseract.js");
            workerRef.current = await createWorker("eng");
          }
          const result = await workerRef.current.recognize(canvas);
          if (acceptRecognized(result.data.text)) return;
        } catch {
          setError(t("nav:openDialog.scanError"));
        } finally {
          recognizingRef.current = false;
        }
      }
      frameId = requestAnimationFrame(scanFrame);
    };
    frameId = requestAnimationFrame(scanFrame);
    return () => cancelAnimationFrame(frameId);
  }, [scanning, t]);

  async function startScanning() {
    setError("");
    try {
      const stream = await navigator.mediaDevices.getUserMedia({
        video: { facingMode: { ideal: "environment" } },
        audio: false,
      });
      streamRef.current = stream;
      setScanning(true);
      requestAnimationFrame(() => {
        if (videoRef.current) {
          videoRef.current.srcObject = stream;
          void videoRef.current.play();
        }
      });
    } catch {
      setError(t("nav:openDialog.cameraError"));
    }
  }

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (!target.trim() || loading) return;
    setLoading(true);
    setError("");
    try {
      const result = await api.openTarget(target.trim());
      stopScanning();
      onClose();
      onBeforeNavigate?.();
      navigate(result.path);
    } catch (e) {
      setError(getErrorMessage(e));
    } finally {
      setLoading(false);
    }
  }

  return (
    <Modal open={open} onClose={onClose} title={t("nav:openDialog.title")}>
      <form onSubmit={submit} className={styles.form}>
        <p className={styles.description}>
          {t(
            scanning ? "nav:openDialog.scanning" : "nav:openDialog.description",
          )}
        </p>
        <div className={styles.inputRow}>
          <div className={styles.inputWithCamera}>
            <input
              autoFocus
              value={target}
              onChange={(event) => setTarget(event.target.value)}
              placeholder={t("nav:openDialog.placeholder")}
              aria-label={t("nav:openDialog.inputLabel")}
            />
            <button
              type="button"
              className={styles.cameraButton}
              onClick={scanning ? stopScanning : startScanning}
              aria-label={t(
                scanning ? "nav:openDialog.stopScan" : "nav:openDialog.scan",
              )}
              title={t(
                scanning ? "nav:openDialog.stopScan" : "nav:openDialog.scan",
              )}
            >
              <svg viewBox="0 0 24 24" aria-hidden="true">
                <path d="M9 4 7.5 6H4a2 2 0 0 0-2 2v10a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-3.5L15 4H9Zm3 13a4 4 0 1 1 0-8 4 4 0 0 1 0 8Zm0-2.2a1.8 1.8 0 1 0 0-3.6 1.8 1.8 0 0 0 0 3.6Z" />
              </svg>
            </button>
          </div>
          <button
            className={styles.submitButton}
            type="submit"
            disabled={!target.trim() || loading}
          >
            {loading ? t("nav:openDialog.opening") : t("nav:openDialog.open")}
          </button>
        </div>
        {error && (
          <p className={styles.error} role="alert">
            {error}
          </p>
        )}
        {scanning && (
          <div className={styles.scanner}>
            <video ref={videoRef} muted playsInline />
            <canvas ref={canvasRef} hidden />
          </div>
        )}
      </form>
    </Modal>
  );
}
