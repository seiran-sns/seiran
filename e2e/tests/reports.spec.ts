import { expect, test } from "@playwright/test";
import { loginViaApi, registerUserViaApi } from "../fixtures/api-helpers";

const ADMIN_USERNAME = "e2ebootstrap";
const ADMIN_PASSWORD = "seiranda-e2e";

test("投稿通報を作成し、管理者が内部コメントとクローズを行える", async ({ request }) => {
  const reporter = await registerUserViaApi(request, "e2ereporter");
  const subject = await registerUserViaApi(request, "e2ereported");
  const noteRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${subject.token}` },
    data: { text: "通報対象E2E", deliver_to_fedi: false, deliver_to_bsky: false },
  });
  expect(noteRes.ok(), await noteRes.text()).toBeTruthy();
  const note = await noteRes.json();

  const createRes = await request.post("/api/reports", {
    headers: { Authorization: `Bearer ${reporter.token}` },
    data: {
      subject_type: "post",
      subject_actor_id: subject.actorId,
      subject_post_id: String(note.id),
      reason_type: "reasonMisleadingSpam",
      reason_text: "E2E通報理由",
    },
  });
  expect(createRes.status(), await createRes.text()).toBe(201);
  const reportId = String((await createRes.json()).id);

  const adminToken = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);
  const headers = { Authorization: `Bearer ${adminToken}` };
  const listRes = await request.get("/api/admin/reports", { headers });
  expect(listRes.ok(), await listRes.text()).toBeTruthy();
  const reports = await listRes.json();
  expect(reports).toEqual(expect.arrayContaining([
    expect.objectContaining({ id: reportId, subject_post_id: String(note.id), reason_type: "reasonMisleadingSpam" }),
  ]));

  const commentRes = await request.post(`/api/admin/reports/${reportId}/comments`, {
    headers, data: { body: "管理者内部メモ" },
  });
  expect(commentRes.status(), await commentRes.text()).toBe(201);
  const closeRes = await request.post(`/api/admin/reports/${reportId}/close`, { headers });
  expect(closeRes.status(), await closeRes.text()).toBe(204);
});
