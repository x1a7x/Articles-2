// static/script.js

document.addEventListener('DOMContentLoaded', () => {
    const images = document.querySelectorAll('.toggle-image');

    images.forEach(img => {
        img.addEventListener('click', () => {
            img.classList.toggle('expanded');
        });
    });
});
